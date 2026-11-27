//! The runtime's COM objects: one process-wide object per class and interface family, whose vtable comes
//! from `classes.txt` (the documented method order) and the implementations each class module offers by
//! method name. A slot nobody implements notes the call in the trace and fails.

use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, c_char, c_void};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use super::{E_NOINTERFACE, E_POINTER, HR_NOT_SUPPORTED};
use crate::code::Code;
use crate::trace::{image_offset, note};

/// A GUID in memory order: the first three fields little-endian, then eight bytes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Guid(pub [u8; 16]);

const IUNKNOWN: Guid = Guid([0, 0, 0, 0, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46]);

impl Guid {
    pub fn parse(text: &str) -> Guid {
        let hex: Vec<u8> = text
            .bytes()
            .filter(|b| *b != b'-')
            .map(|b| (b as char).to_digit(16).unwrap_or(0) as u8)
            .collect();
        let byte = |i: usize| hex[2 * i] << 4 | hex[2 * i + 1];
        let mut out = [0u8; 16];
        for (to, from) in [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15]
            .into_iter()
            .enumerate()
        {
            out[to] = byte(from);
        }
        Guid(out)
    }

    pub fn show(&self) -> String {
        let b = &self.0;
        format!(
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{}",
            b[3],
            b[2],
            b[1],
            b[0],
            b[5],
            b[4],
            b[7],
            b[6],
            b[8],
            b[9],
            b[10..]
                .iter()
                .map(|x| format!("{x:02x}"))
                .collect::<String>()
        )
    }

    /// # Safety
    /// `pointer` is null or points at 16 readable bytes.
    pub unsafe fn read(pointer: *const u8) -> Option<Guid> {
        if pointer.is_null() {
            return None;
        }
        // SAFETY: as the caller promises.
        Some(Guid(unsafe { *(pointer as *const [u8; 16]) }))
    }
}

pub struct Interface {
    pub name: String,
    pub class: usize,
    pub id: Guid,
    pub inherits: Option<usize>,
    pub methods: Vec<Option<String>>,
}

pub struct Class {
    pub name: String,
    pub id: Guid,
    /// Interfaces that inherit from none: each is its own vtable family.
    pub roots: Vec<usize>,
}

pub struct Registry {
    pub classes: Vec<Class>,
    pub interfaces: Vec<Interface>,
}

impl Registry {
    fn parse(text: &str) -> Registry {
        let mut classes: Vec<Class> = Vec::new();
        let mut interfaces: Vec<Interface> = Vec::new();
        let mut parents: Vec<Option<String>> = Vec::new();
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let words: Vec<&str> = line.split_whitespace().collect();
            match words.as_slice() {
                ["class", name, id] => classes.push(Class {
                    name: name.to_string(),
                    id: Guid::parse(id),
                    roots: Vec::new(),
                }),
                ["iface", name, class, id, parent, methods @ ..] => {
                    let class = classes.iter().position(|c| c.name == *class).unwrap_or(0);
                    interfaces.push(Interface {
                        name: name.to_string(),
                        class,
                        id: Guid::parse(id),
                        inherits: None,
                        methods: methods
                            .iter()
                            .map(|m| (*m != "_").then(|| m.to_string()))
                            .collect(),
                    });
                    parents.push((*parent != "-").then(|| parent.to_string()));
                }
                _ => {}
            }
        }
        for (i, parent) in parents.iter().enumerate() {
            interfaces[i].inherits = parent
                .as_ref()
                .and_then(|p| interfaces.iter().position(|x| x.name == *p));
            if interfaces[i].inherits.is_none() {
                let class = interfaces[i].class;
                classes[class].roots.push(i);
            }
        }
        Registry {
            classes,
            interfaces,
        }
    }

    fn root_of(&self, mut interface: usize) -> usize {
        while let Some(parent) = self.interfaces[interface].inherits {
            interface = parent;
        }
        interface
    }

    /// The interfaces of one family, base first, ending at the most derived one.
    pub fn chain(&self, root: usize) -> Vec<usize> {
        let depth = |mut i: usize| {
            let mut d = 0;
            while let Some(p) = self.interfaces[i].inherits {
                i = p;
                d += 1;
            }
            d
        };
        let top = (0..self.interfaces.len())
            .filter(|i| self.root_of(*i) == root)
            .max_by_key(|i| depth(*i))
            .unwrap_or(root);
        let mut chain = vec![top];
        while let Some(parent) = self.interfaces[*chain.last().unwrap_or(&top)].inherits {
            chain.push(parent);
        }
        chain.reverse();
        chain
    }

    /// The method names of a family by slot, from slot 3.
    pub fn slots(&self, root: usize) -> Vec<Option<&str>> {
        self.chain(root)
            .into_iter()
            .flat_map(|i| self.interfaces[i].methods.iter().map(|m| m.as_deref()))
            .collect()
    }

    #[cfg(test)]
    fn class_named(&self, name: &str) -> Option<usize> {
        self.classes.iter().position(|c| c.name == name)
    }

    /// The family a request for `interface` of class `class` is answered from.
    fn family(&self, class: usize, interface: &Guid) -> Option<usize> {
        let classes = &self.classes[class];
        if *interface == IUNKNOWN || *interface == classes.id {
            return classes
                .roots
                .iter()
                .copied()
                .find(|r| self.interfaces[*r].id == classes.id)
                .or_else(|| classes.roots.first().copied());
        }
        self.interfaces
            .iter()
            .position(|i| i.class == class && i.id == *interface)
            .map(|i| self.root_of(i))
    }
}

pub fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| Registry::parse(include_str!("classes.txt")))
}

/// One method a class module provides, by the name the layout gives it, or `@<slot>` for a slot with no name.
pub struct Method {
    pub name: &'static str,
    pub address: usize,
}

/// What a module provides for one family of a class.
pub struct ClassImpl {
    pub class: &'static str,
    pub root: &'static str,
    pub methods: Vec<Method>,
    /// What a slot nobody implements answers.
    pub failure: i32,
}

macro_rules! method {
    ($name:expr, $function:expr) => {
        $crate::loader::com::Method {
            name: $name,
            address: $function as *const () as usize,
        }
    };
}
pub(super) use method;

fn impls() -> &'static [ClassImpl] {
    static IMPLS: OnceLock<Vec<ClassImpl>> = OnceLock::new();
    IMPLS.get_or_init(|| {
        let all: Vec<ClassImpl> = Vec::new();
        all
    })
}

/// Whether `QueryApiImpl` serves the class.
pub fn served(class: &str) -> bool {
    impls().iter().any(|i| i.class == class)
}

#[repr(C)]
struct Object {
    vtable: *const usize,
    refs: AtomicU32,
    class: usize,
    root: usize,
    imp: usize,
}

static OBJECTS: Mutex<Option<HashMap<(usize, usize), usize>>> = Mutex::new(None);
static NOTED: Mutex<Option<HashSet<(usize, usize)>>> = Mutex::new(None);

const TRAP_SLOTS: usize = 128;

fn trap_thunks() -> &'static [usize] {
    static THUNKS: OnceLock<Vec<usize>> = OnceLock::new();
    THUNKS.get_or_init(|| {
        let mut code = Code::default();
        let thunks = (0..TRAP_SLOTS)
            .map(|slot| {
                code.method_trap(slot, trapped as *const () as usize)
                    .unwrap_or(0)
            })
            .collect();
        if code.seal().is_ok() {
            std::mem::forget(code);
        }
        thunks
    })
}

/// # Safety
/// `this` is an `Object` made here.
unsafe extern "win64" fn trapped(this: *const Object, slot: usize, caller: usize) -> i32 {
    // SAFETY: as the caller promises.
    let object = unsafe { &*this };
    let first = NOTED
        .lock()
        .map(|mut seen| {
            seen.get_or_insert_with(HashSet::new)
                .insert((object.class, slot))
        })
        .unwrap_or(false);
    let imp = &impls()[object.imp];
    if first {
        let registry = registry();
        let name = slot
            .checked_sub(3)
            .and_then(|i| registry.slots(object.root).get(i).copied().flatten())
            .unwrap_or("unnamed");
        note(format!(
            "{} slot {slot} ({name}) called from {}; not implemented",
            registry.classes[object.class].name,
            image_offset(caller)
        ));
    }
    imp.failure
}

/// # Safety
/// `riid` is null or points at an IID, `out` is null or writable.
unsafe extern "win64" fn query_interface(
    this: *const Object,
    riid: *const u8,
    out: *mut *mut c_void,
) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: as the caller promises.
    let (object, wanted) = unsafe { (&*this, Guid::read(riid)) };
    let Some(wanted) = wanted else {
        // SAFETY: `out` is writable, checked above.
        unsafe { *out = std::ptr::null_mut() };
        return E_POINTER;
    };
    answer(object.class, &wanted, out)
}

/// # Safety
/// `this` is an `Object` made here.
unsafe extern "win64" fn add_ref(this: *const Object) -> u32 {
    // SAFETY: as the caller promises.
    unsafe { (*this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

/// # Safety
/// `this` is an `Object` made here. The objects are process-wide, so the last reference is never dropped.
unsafe extern "win64" fn release(this: *const Object) -> u32 {
    // SAFETY: as the caller promises.
    let refs = unsafe { &(*this).refs };
    let mut now = refs.load(Ordering::Relaxed);
    while now > 1 {
        match refs.compare_exchange_weak(now, now - 1, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return now - 1,
            Err(seen) => now = seen,
        }
    }
    1
}

fn build(class: usize, root: usize) -> Option<usize> {
    let registry = registry();
    let (imp_index, imp) = impls().iter().enumerate().find(|(_, i)| {
        i.class == registry.classes[class].name && i.root == registry.interfaces[root].name
    })?;
    let slots = registry.slots(root);
    let thunks = trap_thunks();
    let mut table = vec![
        query_interface as *const () as usize,
        add_ref as *const () as usize,
        release as *const () as usize,
    ];
    for (i, name) in slots.iter().enumerate() {
        let slot = 3 + i;
        let by_slot = format!("@{slot}");
        let first = name.is_some_and(|n| slots.iter().position(|s| *s == Some(n)) == Some(i));
        let found = imp
            .methods
            .iter()
            .find(|m| m.name == by_slot || (first && Some(m.name) == *name));
        table.push(
            found
                .map(|m| m.address)
                .or_else(|| thunks.get(slot).copied())
                .unwrap_or(0),
        );
    }
    let vtable = Box::leak(table.into_boxed_slice()).as_ptr();
    let object = Box::new(Object {
        vtable,
        refs: AtomicU32::new(1),
        class,
        root,
        imp: imp_index,
    });
    Some(Box::into_raw(object) as usize)
}

fn object_for(class: usize, root: usize) -> Option<usize> {
    let mut objects = OBJECTS.lock().ok()?;
    let objects = objects.get_or_insert_with(HashMap::new);
    if let Some(found) = objects.get(&(class, root)) {
        return Some(*found);
    }
    let made = build(class, root)?;
    objects.insert((class, root), made);
    Some(made)
}

fn answer(class: usize, interface: &Guid, out: *mut *mut c_void) -> i32 {
    let registry = registry();
    // SAFETY: `out` is writable; every caller checked it.
    unsafe { *out = std::ptr::null_mut() };
    let Some(root) = registry.family(class, interface) else {
        note(format!(
            "{} asked for unknown interface {}",
            registry.classes[class].name,
            interface.show()
        ));
        return E_NOINTERFACE;
    };
    let Some(object) = object_for(class, root) else {
        note(format!(
            "{} interface family {} is not served",
            registry.classes[class].name, registry.interfaces[root].name
        ));
        return E_NOINTERFACE;
    };
    // SAFETY: `object` was made by `build` and is never freed.
    unsafe {
        (*(object as *const Object))
            .refs
            .fetch_add(1, Ordering::Relaxed);
        *out = object as *mut c_void;
    }
    0
}

/// `QueryApiImpl`: the object for a class and an interface of it. A class nobody serves is refused with the
/// error the GDK documents for it, and noted.
pub fn query(class: &Guid, interface: &Guid, out: *mut *mut c_void) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: `out` is writable, checked above.
    unsafe { *out = std::ptr::null_mut() };
    let registry = registry();
    let found = registry.classes.iter().position(|c| c.id == *class);
    let Some(index) = found.filter(|i| served(&registry.classes[*i].name)) else {
        note(format!(
            "QueryApiImpl(class {}, interface {}): not provided",
            class.show(),
            interface.show()
        ));
        return HR_NOT_SUPPORTED;
    };
    answer(index, interface, out)
}

/// The class named `name`, by its index in the registry.
#[cfg(test)]
pub fn class_index(name: &str) -> Option<usize> {
    registry().class_named(name)
}

/// # Safety
/// `text` is null or a NUL-terminated string.
pub unsafe fn c_str(text: *const u8) -> Option<String> {
    if text.is_null() {
        return None;
    }
    // SAFETY: as the caller promises.
    Some(
        unsafe { CStr::from_ptr(text as *const c_char) }
            .to_string_lossy()
            .into_owned(),
    )
}

/// Writes `text` and a NUL into a buffer of `capacity` bytes; false when it does not fit.
///
/// # Safety
/// `buffer` is null or writable for `capacity` bytes.
pub unsafe fn write_c_str(buffer: *mut u8, capacity: usize, text: &str) -> bool {
    if buffer.is_null() || capacity < text.len() + 1 {
        return false;
    }
    // SAFETY: `capacity` bytes are writable and the text and its NUL fit.
    unsafe {
        std::ptr::copy_nonoverlapping(text.as_ptr(), buffer, text.len());
        *buffer.add(text.len()) = 0;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guids_round_trip_through_memory_order() {
        let g = Guid::parse("073b7dcb-1fcf-4030-94be-e3c9eb623428");
        assert_eq!(g.0[..4], [0xcb, 0x7d, 0x3b, 0x07]);
        assert_eq!(g.show(), "073b7dcb-1fcf-4030-94be-e3c9eb623428");
    }

    #[test]
    fn the_layout_has_the_measured_slots() {
        let registry = registry();
        let threading = class_index("XThreadingImpl").unwrap();
        let root = registry.classes[threading].roots[0];
        let slots = registry.slots(root);
        assert_eq!(slots.len(), 28);
        assert_eq!(slots[0], Some("XAsyncGetStatus"));
        assert_eq!(slots[27], Some("XThreadIsTimeSensitive"));
        let package = class_index("XPackageImpl").unwrap();
        let root = registry.classes[package].roots[0];
        assert_eq!(registry.slots(root).len(), 39);
        let user = class_index("XUserImpl").unwrap();
        assert_eq!(registry.classes[user].roots.len(), 2);
    }
}
