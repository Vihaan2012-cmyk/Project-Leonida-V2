use super::ModelError;

const SYSTEM_BASE: u64 = 0x5000_0000;
const GRAPHICS_BASE: u64 = 0x6000_0000;
const WINDOW: u64 = 0x1000_0000;

/// The joined system and graphics pages of one resource. Pointers inside the system pages are
/// `0x5000_0000`-based offsets into `system`; pointers to vertex and index data are
/// `0x6000_0000`-based offsets into `graphics`.
#[derive(Clone, Copy)]
pub(crate) struct Pages<'a> {
    pub system: &'a [u8],
    pub graphics: &'a [u8],
}

impl<'a> Pages<'a> {
    fn bytes(&self, at: usize, len: usize, what: &'static str) -> Result<&'a [u8], ModelError> {
        at.checked_add(len)
            .and_then(|end| self.system.get(at..end))
            .ok_or(ModelError::Bounds { what })
    }

    pub fn u8(&self, at: usize, what: &'static str) -> Result<u8, ModelError> {
        Ok(self.bytes(at, 1, what)?[0])
    }

    pub fn u16(&self, at: usize, what: &'static str) -> Result<u16, ModelError> {
        let b = self.bytes(at, 2, what)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&self, at: usize, what: &'static str) -> Result<u32, ModelError> {
        let b = self.bytes(at, 4, what)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn u64(&self, at: usize, what: &'static str) -> Result<u64, ModelError> {
        let b = self.bytes(at, 8, what)?;
        Ok(u64::from_le_bytes(b.try_into().expect("8 bytes")))
    }

    pub fn f32(&self, at: usize, what: &'static str) -> Result<f32, ModelError> {
        Ok(f32::from_bits(self.u32(at, what)?))
    }

    pub fn f32s<const N: usize>(
        &self,
        at: usize,
        what: &'static str,
    ) -> Result<[f32; N], ModelError> {
        let mut out = [0.0; N];
        for (i, v) in out.iter_mut().enumerate() {
            *v = self.f32(at + 4 * i, what)?;
        }
        Ok(out)
    }

    pub fn slice(&self, at: usize, len: usize, what: &'static str) -> Result<&'a [u8], ModelError> {
        self.bytes(at, len, what)
    }

    pub fn slice_graphics(&self, at: usize, len: usize) -> Result<&'a [u8], ModelError> {
        at.checked_add(len)
            .and_then(|end| self.graphics.get(at..end))
            .ok_or(ModelError::Bounds {
                what: "graphics data",
            })
    }

    /// True when `value` is a pointer into the system pages.
    pub fn is_system_pointer(value: u64) -> bool {
        (SYSTEM_BASE..SYSTEM_BASE + WINDOW).contains(&value)
    }

    /// The pointer stored at `field`, or `None` when it is null.
    pub fn optional_pointer(
        &self,
        field: usize,
        what: &'static str,
    ) -> Result<Option<usize>, ModelError> {
        let value = self.u64(field, what)?;
        if value == 0 {
            return Ok(None);
        }
        self.resolve(value, what).map(Some)
    }

    /// The system offset the pointer stored at `field` names; `size` bytes must fit behind it.
    pub fn pointer(
        &self,
        field: usize,
        size: usize,
        what: &'static str,
    ) -> Result<usize, ModelError> {
        let value = self.u64(field, what)?;
        let at = self.resolve(value, what)?;
        self.bytes(at, size, what)?;
        Ok(at)
    }

    fn resolve(&self, value: u64, what: &'static str) -> Result<usize, ModelError> {
        if !Self::is_system_pointer(value) {
            return Err(ModelError::Pointer { what, value });
        }
        let at = (value - SYSTEM_BASE) as usize;
        if at > self.system.len() {
            return Err(ModelError::Bounds { what });
        }
        Ok(at)
    }

    /// The graphics-page offset a pointer field names, with `len` bytes behind it.
    pub fn graphics_pointer(
        &self,
        field: usize,
        len: usize,
        what: &'static str,
    ) -> Result<usize, ModelError> {
        let value = self.u64(field, what)?;
        if !(GRAPHICS_BASE..GRAPHICS_BASE + WINDOW).contains(&value) {
            return Err(ModelError::Pointer { what, value });
        }
        let at = (value - GRAPHICS_BASE) as usize;
        match at.checked_add(len) {
            Some(end) if end <= self.graphics.len() => Ok(at),
            _ => Err(ModelError::Bounds { what }),
        }
    }

    /// A NUL-terminated string behind the pointer at `field`, or `None` when it is null or unreadable.
    pub fn c_string(&self, field: usize) -> Option<String> {
        let value = self.u64(field, "string").ok()?;
        if !Self::is_system_pointer(value) {
            return None;
        }
        let at = (value - SYSTEM_BASE) as usize;
        let window = self.system.get(at..)?;
        let window = &window[..window.len().min(256)];
        let end = window.iter().position(|&b| b == 0)?;
        let text = &window[..end];
        if text.is_empty() || !text.iter().all(|b| (0x20..0x7F).contains(b)) {
            return None;
        }
        Some(String::from_utf8_lossy(text).into_owned())
    }
}
