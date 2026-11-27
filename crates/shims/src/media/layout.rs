//! The frame buffer description (`BINKFRAMEBUFFERS`) the game reads with `BinkGetFrameBuffersInfo` and hands
//! back with `BinkRegisterFrameBuffers`, as measured from how the executable uses it (see the module docs).
//!
//! ```text
//! +0x00 u32 TotalFrames        frame buffer sets; the game keeps three fences, so at most 3
//! +0x04 u32 YABufferWidth      Y and alpha plane size
//! +0x08 u32 YABufferHeight
//! +0x0C u32 cRcBBufferWidth    chroma plane size
//! +0x10 u32 cRcBBufferHeight
//! +0x14 u32 FrameNum           the set holding the newest frame; written by us after each decoded frame
//! +0x18 set[3], 0x50 bytes each: five planes of 0x10 bytes (Y, cR, cB, A, H):
//!         +0x00 u64 Buffer     the game's mapped texture memory (0 until it fills it in)
//!         +0x08 u32 Allocate   nonzero when the plane exists (we set it; the game tests it)
//!         +0x0C u32 Pitch      row pitch in bytes of Buffer (the game sets it)
//! ```
//! The whole description is 0x108 bytes. Every plane is one byte per pixel.

use super::decoder::{DecodedFrame, StreamInfo};

pub const SIZE: usize = 0x108;
pub const TOTAL_FRAMES: u32 = 3;
const SETS: usize = 0x18;
const SET_BYTES: usize = 0x50;
const PLANE_BYTES: usize = 0x10;
const PLANES: usize = 5;

const Y: usize = 0;
const CR: usize = 1;
const CB: usize = 2;
const ALPHA: usize = 3;

fn plane_at(set: u32, plane: usize) -> usize {
    SETS + set as usize * SET_BYTES + plane * PLANE_BYTES
}

/// The size of the chroma planes of a `width` x `height` movie.
pub fn chroma_size(width: u32, height: u32) -> (u32, u32) {
    (width.div_ceil(2), height.div_ceil(2))
}

/// Writes the description of a movie: the plane sizes, `Allocate` set on Y, cR, cB (and alpha when the movie
/// has it), and no buffers yet.
///
/// # Safety
/// `out` must be valid for [`SIZE`] bytes of writes.
pub unsafe fn describe(out: *mut u8, info: &StreamInfo) {
    let (cw, ch) = chroma_size(info.width, info.height);
    // SAFETY: the caller's guarantee covers every offset below `SIZE`.
    unsafe {
        std::ptr::write_bytes(out, 0, SIZE);
        let put = |at: usize, value: u32| (out.add(at) as *mut u32).write_unaligned(value);
        put(0x00, TOTAL_FRAMES);
        put(0x04, info.width);
        put(0x08, info.height);
        put(0x0C, cw);
        put(0x10, ch);
        for set in 0..TOTAL_FRAMES {
            for plane in [Y, CR, CB] {
                put(plane_at(set, plane) + 8, 1);
            }
            if info.has_alpha {
                put(plane_at(set, ALPHA) + 8, 1);
            }
        }
    }
}

unsafe fn get(base: *const u8, at: usize) -> u32 {
    // SAFETY: the callers pass offsets inside the description.
    unsafe { (base.add(at) as *const u32).read_unaligned() }
}

/// The whole frame buffer sets the game registered, as far as they can be written.
struct Target {
    buffer: *mut u8,
    pitch: usize,
    width: usize,
    height: usize,
}

unsafe fn target(desc: *const u8, set: u32, plane: usize) -> Option<Target> {
    let at = plane_at(set, plane);
    // SAFETY: inside the description.
    let (buffer, allocate, pitch) = unsafe {
        (
            (desc.add(at) as *const u64).read_unaligned() as usize,
            get(desc, at + 8),
            get(desc, at + 12),
        )
    };
    if buffer == 0 || allocate == 0 || pitch == 0 {
        return None;
    }
    let chroma = plane == CR || plane == CB;
    // SAFETY: inside the description.
    let (width, height) = unsafe {
        if chroma {
            (get(desc, 0x0C), get(desc, 0x10))
        } else {
            (get(desc, 0x04), get(desc, 0x08))
        }
    };
    Some(Target {
        buffer: buffer as *mut u8,
        pitch: pitch as usize,
        width: width as usize,
        height: height as usize,
    })
}

/// Copies the planes of `frame` into frame buffer set `set` and then makes `set` the current one.
///
/// Rows are copied at the smaller of the decoded plane's, the texture's and the pitch's width, and only for
/// as many rows as both have; an alpha plane the decoder did not produce is filled opaque.
///
/// # Safety
/// `desc` must be a description whose registered buffers are valid for `pitch * height` bytes of writes
/// for each plane with its `Allocate` word set.
pub unsafe fn fill(desc: *mut u8, frame: &DecodedFrame, set: u32) {
    for plane in [Y, CR, CB, ALPHA] {
        // SAFETY: the caller's guarantee.
        let Some(t) = (unsafe { target(desc, set, plane) }) else {
            continue;
        };
        match frame.planes.get(plane) {
            Some(src) => {
                let width = src.width.min(t.width).min(t.pitch).min(src.stride);
                let rows = src.height.min(t.height);
                for row in 0..rows {
                    let Some(line) = src.data.get(row * src.stride..row * src.stride + width)
                    else {
                        break;
                    };
                    // SAFETY: `row < height` and `width <= pitch`, within the buffer the caller vouched for.
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            line.as_ptr(),
                            t.buffer.add(row * t.pitch),
                            width,
                        )
                    };
                }
            }
            None if plane == ALPHA => {
                for row in 0..t.height {
                    // SAFETY: as above.
                    unsafe {
                        std::ptr::write_bytes(
                            t.buffer.add(row * t.pitch),
                            0xFF,
                            t.width.min(t.pitch),
                        )
                    };
                }
            }
            None => {}
        }
    }
    // SAFETY: inside the description.
    unsafe { (desc.add(0x14) as *mut u32).write_volatile(set) };
}

/// Fills every set with a black frame (Y 0, chroma 128, alpha opaque) and makes set 0 current.
///
/// # Safety
/// As [`fill`].
pub unsafe fn fill_black(desc: *mut u8) {
    for set in 0..TOTAL_FRAMES {
        for plane in [Y, CR, CB, ALPHA] {
            // SAFETY: the caller's guarantee.
            let Some(t) = (unsafe { target(desc, set, plane) }) else {
                continue;
            };
            let value = if plane == CR || plane == CB {
                0x80
            } else if plane == Y {
                0
            } else {
                0xFF
            };
            for row in 0..t.height {
                // SAFETY: `row < height`, and the row is at most one pitch.
                unsafe {
                    std::ptr::write_bytes(t.buffer.add(row * t.pitch), value, t.width.min(t.pitch))
                };
            }
        }
    }
    // SAFETY: inside the description.
    unsafe { (desc.add(0x14) as *mut u32).write_volatile(0) };
}

/// Whether the game's description has a buffer for the Y plane of set 0, which is what makes it worth filling.
///
/// # Safety
/// `desc` must be valid for [`SIZE`] bytes of reads.
pub unsafe fn registered(desc: *const u8) -> bool {
    // SAFETY: the caller's guarantee.
    unsafe { target(desc, 0, Y).is_some() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::decoder::MovieDecoder;
    use crate::media::testing::FakeDecoder;

    /// A description plus the plane memory the game would have mapped, with a pitch wider than the texture.
    struct Game {
        desc: Vec<u64>,
        planes: Vec<Vec<u8>>,
    }

    const PITCH: usize = 64;

    impl Game {
        fn new(info: &StreamInfo) -> Game {
            let mut desc = vec![0u64; SIZE / 8];
            // SAFETY: `desc` is SIZE bytes.
            unsafe { describe(desc.as_mut_ptr() as *mut u8, info) };
            let mut game = Game {
                desc,
                planes: Vec::new(),
            };
            for set in 0..TOTAL_FRAMES {
                for plane in 0..PLANES {
                    let at = plane_at(set, plane);
                    // SAFETY: inside `desc`.
                    let allocate = unsafe { get(game.base(), at + 8) };
                    if allocate == 0 {
                        continue;
                    }
                    let h = if plane == CR || plane == CB {
                        chroma_size(info.width, info.height).1
                    } else {
                        info.height
                    } as usize;
                    game.planes.push(vec![0x55; PITCH * h]);
                    let ptr = game.planes.last_mut().unwrap().as_mut_ptr() as u64;
                    let bytes = game.bytes_mut();
                    bytes[at..at + 8].copy_from_slice(&ptr.to_le_bytes());
                    bytes[at + 12..at + 16].copy_from_slice(&(PITCH as u32).to_le_bytes());
                }
            }
            game
        }

        fn base(&self) -> *const u8 {
            self.desc.as_ptr() as *const u8
        }

        fn bytes_mut(&mut self) -> &mut [u8] {
            // SAFETY: `desc` is SIZE bytes of u64.
            unsafe { std::slice::from_raw_parts_mut(self.desc.as_mut_ptr() as *mut u8, SIZE) }
        }

        fn word(&self, at: usize) -> u32 {
            // SAFETY: inside `desc`.
            unsafe { get(self.base(), at) }
        }

        /// The memory behind (set, plane) in the order `new` made it.
        fn plane(&self, set: u32, plane: usize, alpha: bool) -> &Vec<u8> {
            let per_set = if alpha { 4 } else { 3 };
            &self.planes[set as usize * per_set + plane]
        }
    }

    fn info(w: u32, h: u32, alpha: bool) -> StreamInfo {
        FakeDecoder::new(10, w, h, 0, alpha).info
    }

    #[test]
    fn the_description_has_the_measured_shape() {
        let game = Game::new(&info(100, 60, false));
        assert_eq!(SIZE, 0x18 + 3 * 0x50);
        assert_eq!(game.word(0x00), 3);
        assert_eq!((game.word(0x04), game.word(0x08)), (100, 60));
        assert_eq!((game.word(0x0C), game.word(0x10)), (50, 30));
        assert_eq!(game.word(0x14), 0);
        for set in 0..3 {
            let flags: Vec<u32> = (0..5).map(|p| game.word(plane_at(set, p) + 8)).collect();
            assert_eq!(flags, [1, 1, 1, 0, 0], "set {set}");
        }
        let alpha = Game::new(&info(100, 60, true));
        assert_eq!(alpha.word(plane_at(1, 3) + 8), 1);
        assert_eq!(
            alpha.word(plane_at(1, 4) + 8),
            0,
            "the fifth plane is never asked for"
        );
    }

    #[test]
    fn odd_sizes_round_the_chroma_planes_up() {
        let game = Game::new(&info(1407, 1601, false));
        assert_eq!((game.word(0x0C), game.word(0x10)), (704, 801));
    }

    #[test]
    fn a_frame_lands_in_its_set_at_the_games_pitch_and_names_it_current() {
        let i = info(40, 24, true);
        let mut game = Game::new(&i);
        let mut decoder = FakeDecoder::new(10, 40, 24, 0, true);
        decoder.decode_next().unwrap();
        let frame = decoder.decode_next().unwrap();
        // SAFETY: the game's buffers are PITCH * height bytes.
        unsafe { fill(game.desc.as_mut_ptr() as *mut u8, &frame, 2) };
        assert_eq!(game.word(0x14), 2);
        for p in 0..4 {
            let src = &frame.planes[p];
            let memory = game.plane(2, p, true);
            for y in 0..src.height {
                for x in 0..src.width {
                    assert_eq!(
                        memory[y * PITCH + x],
                        FakeDecoder::pixel(p, 1, x, y),
                        "plane {p} at {x},{y}"
                    );
                }
                assert!(
                    memory[y * PITCH + src.width..(y + 1) * PITCH]
                        .iter()
                        .all(|&b| b == 0x55)
                );
            }
            assert!(
                game.plane(0, p, true).iter().all(|&b| b == 0x55),
                "other sets are left alone"
            );
        }
    }

    #[test]
    fn planes_wider_than_the_texture_are_cut_and_missing_alpha_is_opaque() {
        let i = info(20, 10, true);
        let mut game = Game::new(&i);
        let mut decoder = FakeDecoder::new(10, 30, 14, 0, false);
        let frame = decoder.decode_next().unwrap();
        // SAFETY: as above.
        unsafe { fill(game.desc.as_mut_ptr() as *mut u8, &frame, 0) };
        let y = game.plane(0, 0, true);
        assert_eq!(y[9 * PITCH + 19], FakeDecoder::pixel(0, 0, 19, 9));
        assert_eq!(y[9 * PITCH + 20], 0x55, "nothing past the texture's width");
        let alpha = game.plane(0, 3, true);
        assert!(
            alpha[..10 * PITCH]
                .chunks(PITCH)
                .all(|r| r[..20].iter().all(|&b| b == 0xFF))
        );
    }

    #[test]
    fn black_is_zero_luma_and_neutral_chroma() {
        let mut game = Game::new(&info(16, 16, true));
        // SAFETY: as above.
        unsafe {
            assert!(registered(game.base()));
            fill_black(game.desc.as_mut_ptr() as *mut u8);
        }
        assert_eq!(game.plane(1, 0, true)[0], 0);
        assert_eq!(game.plane(1, 1, true)[0], 0x80);
        assert_eq!(game.plane(1, 2, true)[7 * PITCH + 7], 0x80);
        assert_eq!(game.plane(1, 3, true)[0], 0xFF);
        assert_eq!(
            game.plane(1, 0, true)[20],
            0x55,
            "padding past the texture is not touched"
        );
    }

    #[test]
    fn unregistered_buffers_are_skipped() {
        let mut desc = vec![0u64; SIZE / 8];
        // SAFETY: `desc` is SIZE bytes.
        unsafe {
            describe(desc.as_mut_ptr() as *mut u8, &info(16, 16, false));
            assert!(!registered(desc.as_ptr() as *const u8));
            let mut decoder = FakeDecoder::new(2, 16, 16, 0, false);
            fill(
                desc.as_mut_ptr() as *mut u8,
                &decoder.decode_next().unwrap(),
                1,
            );
            fill_black(desc.as_mut_ptr() as *mut u8);
        }
    }
}
