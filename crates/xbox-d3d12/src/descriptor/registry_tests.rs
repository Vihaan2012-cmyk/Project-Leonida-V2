use shader_translate::images::{self, Number};

use super::gfx10::*;
use super::registry::*;

fn texture(format: u32, address: u64) -> ImageWords {
    encode_image(&Image {
        address,
        format,
        width: 64,
        height: 64,
        ..Image::default()
    })
}

fn sampler(linear: bool) -> SamplerWords {
    let filter = if linear {
        Filter::Bilinear
    } else {
        Filter::Point
    };
    encode_sampler(&Sampler {
        mag: filter,
        min: filter,
        ..Sampler::default()
    })
}

#[test]
fn a_registered_image_is_found_by_the_shaders_lookup() {
    let mut registry = Registry::new();
    let a = texture(56, 0x1_0000_0000);
    let b = texture(20, 0x1_0000_1000);
    let c = texture(21, 0x1_0000_2000);
    let first = registry.register_texture(&a).unwrap();
    assert_eq!(
        first,
        Registration {
            index: 1,
            new: true
        }
    );
    registry.register_texture(&b).unwrap();
    registry.register_texture(&c).unwrap();
    let tables = registry.tables().unwrap().clone();
    assert_eq!(
        images::lookup(&tables.textures, &a),
        images::slot(1, Number::Float)
    );
    assert_eq!(
        images::lookup(&tables.textures, &b),
        images::slot(2, Number::Uint)
    );
    assert_eq!(
        images::lookup(&tables.textures, &c),
        images::slot(3, Number::Sint)
    );
    assert_eq!(
        images::lookup(&tables.textures, &texture(56, 0x2_0000_0000)),
        0
    );
}

#[test]
fn registering_twice_gives_the_same_slot() {
    let mut registry = Registry::new();
    let a = texture(56, 0x1_0000_0000);
    let once = registry.register_texture(&a).unwrap();
    let again = registry.register_texture(&a).unwrap();
    assert_eq!(once.index, again.index);
    assert!(once.new && !again.new);
    assert_eq!(registry.texture_count(), 1);
    assert_eq!(registry.texture(once.index).unwrap().image.width, 64);
}

#[test]
fn samplers_are_found_by_their_four_words() {
    let mut registry = Registry::new();
    let (point, linear) = (sampler(false), sampler(true));
    let p = registry.register_sampler(&point);
    let l = registry.register_sampler(&linear);
    assert_ne!(p.index, l.index);
    let tables = registry.tables().unwrap().clone();
    let key = |s: &SamplerWords| images::sampler_key(s);
    assert_eq!(
        images::lookup(&tables.samplers, &key(&point)),
        images::slot(p.index, Number::Float)
    );
    assert_eq!(
        images::lookup(&tables.samplers, &key(&linear)),
        images::slot(l.index, Number::Float)
    );
    assert_eq!(images::lookup(&tables.samplers, &key(&[1, 2, 3, 4])), 0);
    assert!(registry.sampler(l.index).unwrap().sampler.mag.linear());
}

#[test]
fn a_released_slot_is_reused_and_leaves_the_table() {
    let mut registry = Registry::new();
    let a = texture(56, 0x1_0000_0000);
    let b = texture(56, 0x1_0000_1000);
    let c = texture(56, 0x1_0000_2000);
    registry.register_texture(&a).unwrap();
    let kept = registry.register_texture(&b).unwrap();
    assert!(registry.release_texture(&a));
    assert!(!registry.release_texture(&a));
    let tables = registry.tables().unwrap().clone();
    assert_eq!(images::lookup(&tables.textures, &a), 0);
    assert_ne!(images::lookup(&tables.textures, &b), 0);
    let reused = registry.register_texture(&c).unwrap();
    assert_eq!(reused.index, 1);
    assert_eq!(
        registry.texture_slot(&b),
        images::slot(kept.index, Number::Float)
    );
    assert_eq!(registry.texture_slot(&a), 0);
}

#[test]
fn tables_are_rebuilt_only_after_a_change() {
    let mut registry = Registry::new();
    registry
        .register_texture(&texture(56, 0x1_0000_0000))
        .unwrap();
    let first = registry.tables().unwrap().version;
    assert_eq!(registry.tables().unwrap().version, first);
    registry
        .register_texture(&texture(56, 0x1_0000_0000))
        .unwrap();
    assert_eq!(registry.tables().unwrap().version, first);
    registry.register_sampler(&sampler(true));
    assert_ne!(registry.tables().unwrap().version, first);
}

#[test]
fn a_buffer_descriptor_is_not_an_image() {
    let mut registry = Registry::new();
    let v = encode_buffer(&Buffer::default());
    let mut words = [0; 8];
    words[..4].copy_from_slice(&v);
    assert!(matches!(
        registry.register_texture(&words),
        Err(RegistryError::NotAnImage(_))
    ));
}

#[test]
fn a_heap_the_game_wrote_itself_is_scanned() {
    let mut registry = Registry::new();
    let mut heap = vec![0u32; 32 * 4];
    let a = texture(56, 0x1_0000_0000);
    let b = texture(22, 0x1_0000_4000);
    heap[0..8].copy_from_slice(&a);
    heap[64..72].copy_from_slice(&b);
    let mut buffer = [0; 8];
    buffer[..4].copy_from_slice(&encode_buffer(&Buffer {
        address: 0x4000,
        num_records: 16,
        ..Buffer::default()
    }));
    heap[32..40].copy_from_slice(&buffer);
    let found = registry.scan_textures(&heap, 8);
    assert_eq!(
        found.iter().map(|(slot, _)| *slot).collect::<Vec<_>>(),
        vec![0, 8]
    );
    assert_eq!(registry.texture_count(), 2);
    let mut samplers = vec![0u32; 16];
    samplers[4..8].copy_from_slice(&sampler(true));
    let found = registry.scan_samplers(&samplers, 4);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].0, 1);
}
