# Leonida (Rust): design

Date: 2026-11-26
Status: draft, awaiting review
License: GPL-3.0 (this is a rewrite of [ItsNotRexed/Leonida](https://github.com/ItsNotRexed/Leonida), which is GPL-3.0)

## 1. Purpose

Leonida runs the Xbox Series build of GTA VI (package version 1.0.2.1) natively on a PC, built from the player's own
copy. The console CPU is x86-64, so the game's code runs natively; the console's system software is reimplemented,
and its precompiled RDNA2 GPU programs are translated to SPIR-V for Vulkan.

This project is a **clean-break rewrite in Rust** of the upstream project (Python translator and tools, C# asset
exporter, C++ runtime). It keeps upstream's measured knowledge (formats, hardware behaviour, test expectations) and
replaces its code, formats and interfaces.

### Goals, in priority order

1. **Get further toward booting the game**: every part around the game executable works against the real package,
   so the first boot attempt yields a precise to-do list instead of a crash.
2. **Renderer upgrades**: precompiled pipelines (no stutter), internal resolution scaling, filtering overrides now;
   uncapped frame rate and ultrawide once the game runs.
3. **Readable and moddable**: an X-Plane-style folder layout, MSFS-style packages, text formats a person can open.
4. **Speed**: parallel, in-process translation and on-demand asset decoding.
5. **Maintainability**: small crates with one job each, a real shader IR, no global state.
6. **Easy install**: one downloadable `leonida.exe`; no Python, .NET, LLVM or Vulkan SDK to install.

### Constraints (hard rules)

- **No game data in the repository, ever**: no archives, executables, shaders in any form, textures, audio, or logs
  that quote them. Enforced by `.gitignore` and a CI content scan (section 10).
- **No keys, decryption or protection workarounds.** Where a format is encrypted (for example the RPF table of
  contents), we read only plaintext data around it, as upstream does, and document the limit.
- **Offline, no exceptions**: no Xbox Live, Rockstar or other online services. The runtime blocks every way the game
  could reach the network, by one policy in the `offline` crate (section 9.2).
- **Only the player's own copy.** Nothing in Leonida downloads, describes how to obtain, or ships game files.

### Current situation

- The developer has the full package **except the game executable**. Everything up to the boot itself can be built
  and verified against the real archives; booting cannot happen until an executable is available.
- Development GPU: **NVIDIA** (32-wide subgroups). Wave64 emulation is therefore a first-class feature, not a fallback.

### Terminology

| Term | Meaning |
|---|---|
| `leonida.exe` | **Our** program, built from this repo. Contains no game code or data; freely distributable. |
| game executable | The game's own `.exe` from the player's package. Never distributed; loaded by `leonida.exe` at run time. |
| package | The player's game folder (`MicrosoftGame.Config`, `.rpf` archives, DLLs). Read-only to Leonida. |
| Leonida package | A folder with a `manifest.json` that adds or overrides content in the virtual file system (a mod, or Leonida's own fixes). |

## 2. Architecture

One Cargo workspace. Each crate has one job; no source file should pass roughly 1,500 lines without being split.

| Crate | Job | Platform |
|---|---|---|
| `leonida` (bin) | The only executable: guided setup and all subcommands | all |
| `game-package` | Find the player's package, recognise its version, check its files, map files read-only | all |
| `rdna` | RDNA2 instruction tables, decoder and assembler (one shared table) | all |
| `shader-container` | Locate shaders in `shaders.rpf`; unpack DXBC → XBCH → FSSM; parse the FSSM header | all |
| `shader-ir` | SSA IR with explicit wave semantics; CFG construction and structurization | all |
| `shader-translate` | Lift RDNA2 → IR, passes, emit SPIR-V, validate | all |
| `shader-store` | `redb` shader store, overrides through packages, export | all |
| `spirv-text` | SPIR-V assembly text ↔ binary (for `.spvasm` overrides and export) | all |
| `testkit` | CPU reference model of RDNA2 used by translator tests | all |
| `rpf9` | Archive reader (plaintext resources only) and asset index | all |
| `assets` | Decoders: models, texture dictionaries/pools (BCPack → BCn), materials, audio; exporters (glTF, DDS, Ogg) | all |
| `systems` | Game behaviour data (binary XML and tables) ↔ readable XML/JSON, both directions | all |
| `vfs` | Package-based virtual file system; optional ProjFS mount | all (mount: Windows) |
| `gpu` | Vulkan foundation (`ash`, `gpu-allocator`): device, memory, bindless descriptors, pipeline cache | all |
| `render` | Renderer: passes, resource upload, GPU detiling, upscaling | all |
| `scene` | Harness that draws real game models with the game's real translated shaders | all |
| `pe-image` | Read-only executable parsing: sections, imports, exception table, strings | all |
| `decompile` | Readable pseudocode from executable functions, for study only | all |
| `offline` | The offline policy: every DLL, function and path that could reach the network | all |
| `pe-loader` | Maps and links the game executable | Windows |
| `shims` | Reimplemented system/GDK libraries | Windows |
| `xbox-d3d12` | The console's D3D12 interface on top of `render` | Windows |

Workspace rules:

- No global mutable state. Configuration is passed in explicitly.
- `unsafe` is confined to `gpu`, `pe-loader`, `shims` and `xbox-d3d12`, plus one read-only file-mapping helper in
  `game-package` (`map_file`, used by every archive reader) and one font helper in `leonida-ui` (`WM_SETFONT`),
  behind safe wrappers, each block commented with the invariant it relies on.
- Library crates return typed errors (`thiserror`); only the `leonida` binary turns them into user messages.
- Output is deterministic: the same inputs give the same bytes.

### 2.1 Pure Rust

Leonida contains no C or C++ code, and no dependency compiles any. Only a Rust compiler is needed to build it.

| Need | Choice (pure Rust) | Instead of |
|---|---|---|
| Build-state and shader store | `redb` (embedded, transactional, crash-safe key-value store) | SQLite |
| SPIR-V reading and writing | `rspirv` + our own `spirv-text` crate (assembler/disassembler for `.spvasm`) | SPIRV-Tools `spirv-as`/`spirv-dis` |
| SPIR-V validation | Our own checks of everything we emit (IDs, types, structured control flow, capabilities); driver pipeline creation on the player's PC is the final check | SPIRV-Tools `spirv-val` (kept as an optional **development and CI** cross-check only) |
| RDNA2 instruction tables | Generated by a build script from AMD's published machine-readable RDNA2 ISA (XML data) | LLVM tables (`llvm-mc` kept as an optional **development and CI** cross-check only) |
| Compression | `flate2` with its `miniz_oxide` backend; `ruzstd` | zlib, libzstd |
| Our own GPU shaders (detiling, upscaling, BVH builds, test kernels) | Written in WGSL, compiled to SPIR-V by `naga` at build time | HLSL/GLSL with glslang, DXC or shaderc |
| Upscaler | AMD FSR 1 (MIT) ported to WGSL | FSR 2/3 C++ SDK |
| Vulkan | `ash` bindings, `gpu-allocator` | C/C++ wrappers |
| Windows APIs, including Projected File System | `windows` / `windows-sys` crates | C++ |
| Desktop window (`leonida-ui`) | Native Win32 controls through `winsafe`; `rfd` for file dialogs; manifest embedded by `embed-manifest` | Qt, GTK, WinUI |
| Game window and input | `winit`; `gilrs` for controllers; DualSense extras over HID through `windows` | SDL3 |
| Audio output | `cpal` | XAudio2 wrappers in C++ |
| XMA2 decoding | Our own Rust decoder | ffmpeg |
| x86-64 disassembly for code patches | `iced-x86` | C libraries |

What stays outside our code and is never compiled by us: the GPU driver and the Vulkan loader (system components),
and the optional development cross-checks above (`spirv-val`, `llvm-mc`, Vulkan validation layers), which are run as
separate programs and never linked or shipped.

## 3. Folder layout (what the player sees)

Modelled on X-Plane 12: folders named after things, self-contained items, open formats, player content layered over
the base game, generated files kept apart.

```
Leonida/
├── leonida.exe
├── Characters/<name>/          <name>.glb, <name>.json, textures/*.dds      (filled by export)
├── Vehicles/<name>/            model, textures/, liveries/, <name>.json
├── World/<area>/               buildings, props, terrain, placements.json
├── Props/
├── Audio/Radio/<station>/<track>.ogg, Audio/Speech/<character>/...
├── Shaders/                    exported .spv / .spvasm (on request)
├── Systems/                    game behaviour as readable text (section 6)
├── Config/                     leonida.toml, graphics.toml, input.toml, mods.toml
├── Mods/<package>/             manifest.json + the same layout; overrides the game (section 7)
└── Output/                     leonida.db, logs/, boot/, crash_reports/, captures/, saves/, runtime/
```

- Asset folders are **filled on demand by export**; the renderer never needs them (section 5).
- Resources with no recovered name go under `_unnamed/<hash>/` so they never clutter the main tree.
- Everything Leonida or the game writes goes to `Output/`.

## 4. Shader translator (Phase 1)

### 4.1 Input

The corpus is 26,503 shaders: 2,236 vertex, 12,101 pixel, 2,689 compute, 9,282 mesh and 195 geometry; 23,841 of
them are wave64 (counts from upstream's README).

`shader-container` reads `shaders.rpf` in place and yields `FssmShader { hash, header, code }`:

- Locate containers by scanning for their plaintext records (the archive's table of contents is encrypted and is not
  read).
- Unpack DXBC → DXIL part → XDAT tag 0x0D → XBCH → zlib → FSSM. Containers with two payloads yield two shaders.
- Parse the FSSM header: stage, wave size, user-SGPR layout, code range. Upstream measured the wave-size flag at
  FSSM offset 0xD4 and the code range from the u32 at 0xC0 plus 0x28; these are re-verified over the whole corpus
  before being relied on.
- Check the container for reflection data (constant-buffer layouts and names) early; the renderer harness depends on
  the answer (section 13, risk 1).

### 4.2 Pipeline

1. **Decode** (`rdna`): bytes → typed `Inst` for every RDNA2 (gfx10.3, `gfx1013` as upstream) encoding family
   (SOP1/SOP2/SOPK/SOPC/SOPP, SMEM, VOP1/VOP2/VOP3/VOP3P/VOPC, DPP/SDWA, DS, MUBUF/MTBUF, MIMG, EXP, FLAT/GLOBAL/
   SCRATCH). Tables are a checked-in data file turned into Rust by a build script; the same table drives the
   assembler used by tests.
2. **Lift**: every register is explicit. SGPRs are wave-uniform values, VGPRs per-lane values, and `EXEC`, `VCC`,
   `SCC` and `M0` are ordinary values. Lifting produces register-variable form; SSA construction follows
   (Braun et al., *Simple and Efficient Construction of SSA Form*).
3. **Passes** (`shader-ir`, `shader-translate`):
   - Divergence recovery: turn exec-mask save/restore patterns back into structured if/else and loops.
   - Structurizer: SPIR-V needs structured control flow; irreducible graphs are first made reducible by node
     splitting and, failing that, lowered to a dispatch loop.
   - **Wave64 lowering** (section 4.3).
   - Cleanup: dead code, constant folding, value numbering.
4. **Emit**: binary SPIR-V through `rspirv`, plus a typed `ShaderInterface` (inputs, outputs, resource bindings,
   push constants, descriptor model).
5. **Validate**: our own checks, in process, of everything the emitter produces: ID definitions and uses, types,
   structured control flow, decorations and capabilities. `spirv-val` runs over the corpus in development and CI as an
   independent cross-check; on the player's PC, driver pipeline creation (`leonida pipelines`) is the final check.

**Pixel shaders.** The program header gives the input enables (offset 0x248), depth control and colour write mask
after them, the user-SGPR count (bits 16-21 of 0xD4) and one two-word entry per interpolated input from 0x138
(index, mask and input-signature register; then the mode, 1 flat or 2 interpolated, with 8 for half precision).
Every shader in the game uses only centre perspective barycentrics plus position and front-face inputs. The
fragment entry preloads v0-v1 from `BaryCoordKHR` (I = y, J = z) and the enabled position and facing inputs after
them, and reads attribute N as the raw words of a `PerVertexKHR` input at location N, so interpolation runs the
shader's own instructions. Colour target N is the output at location N, typed from the output signature; `mrtz`
writes `FragDepth`. Exec starts as the non-helper invocations, and pixels outside exec at the last (`done`) export are
discarded. Wave64 code runs on 32-wide subgroups: fragment shaders have no workgroup to emulate 64 lanes with. The
vertex-side contract (which parameter export feeds which location) is the linker's job and still to come.

**Vertex and mesh shaders.** Vertex shaders are the legacy kind: v0 is the vertex index, v3 the instance index less
the base instance, user SGPRs start at s0, and an `s_swappc` at the first instruction calls the fetch shader. The
entry does the fetch itself: each input-signature element that is not a system value is a four-component Vulkan input
at the location of its register r, loaded into v[4+4r..7+4r]. Mesh shaders are primitive shaders: s3 holds the wave's
index in the group in bits 24 and up, v5 the thread's index across the dispatch, and user SGPRs start at s8. The group
size, vertex and primitive limits and shared memory come from the pipeline-state part. The entry sets its outputs to
the limits at the start and culls every primitive slot; thread T's `pos0` and `paramN` exports fill vertex T, its
`prim` export (three 9-bit indices at bits 0, 10 and 20, bit 31 for none) fills primitive T. In both stages `pos0` is
`Position` and `paramN` the raw words at location N; `pos1`-`pos3` and allocation messages are ignored for now.

**Compute groups.** A compute entry declares a flat group of whole waves and rebuilds the thread IDs from the flat
index, x fastest, as the hardware packs threads into waves. A group that ends partway through a wave starts that wave
with exec holding only the lanes that carry its threads. Implicit-LOD sampling outside pixel shaders takes its
derivatives the way the hardware does: across each quad of lanes, from the first lane to the second horizontally and
to the third vertically, with a bias scaling both by 2^bias, then samples with those gradients.

**Ordered pixel shading.** Shaders that use the hardware's ordered pixel shading set the packer register and spin on
the exiting-wave ID until the waves they overlap have finished. The translator runs the whole fragment entry inside a
pixel interlock instead: the packer write does nothing, the exiting-wave ID reads as zero, and with no overlap
reported in the input SGPRs the wait is skipped. Sample offsets must be constants the same block wrote to the address
register, since Vulkan only takes computed offsets on gathers.

**Images and samplers.** A shader names a texture by its 8-word T# and a sampler by its 4-word S#, both in SGPRs.
The runtime registers every texture and sampler it creates in two lookup tables, and the shader finds the bindless
slot by looking its descriptor up:

- Descriptor set 1: binding 0 is the texture table, binding 1 the sampler table (storage buffers), binding 2 the
  sampler array, then one sampled-image array per dimension and number type: 2D (also 1D) at 3/4/5, 2D array (also
  1D array and cube, as six layers) at 6/7/8, 3D at 9/10/11, for float, unsigned and signed formats. Bindings
  12-20 repeat the same layout as storage images for writes, and binding 21 holds each sampler again with depth
  comparison on (the S#'s DEPTH_COMPARE_FUNC), all at the same indices. Atomics need a fixed format: bindings 22-24
  hold the textures as R32_UINT storage images and 25-27 as R64_UINT (2D, 2D array, 3D), again at the same indices.
- Image loads still read through the sampled views. A texture the same shader also writes would need its reads to
  go through the storage view instead; a spare bit in the slot word is reserved to flag those.
- A table is `mask` followed by `mask + 1` entries of ten words: the descriptor (S# padded with zeros), the slot,
  and a spare word. FNV-1a over the eight key words picks the first entry; up to 16 entries are probed in order. The
  slot's top two bits give the number type, so the shader never decodes the T# format. A descriptor that is not
  found uses slot 0, which the runtime fills with a default texture and sampler.
- The view's mip range and component swizzle come from the T# when the runtime creates it, so the shader only picks
  the slot. `shader_translate::images` holds this contract and builds the tables.

Unsupported instructions are errors carrying the instruction address, never guesses. Each shader returns
`Result<TranslatedShader, TranslateError>`; one failure never stops a run. Translation runs in parallel with `rayon`.

### 4.3 Wave64 on 32-wide GPUs

- **Compute and mesh**: exact. Each logical 64-lane wave runs as two 32-lane subgroups that synchronize through
  workgroup memory for cross-lane operations (ballot, readlane, permutes, DPP, reductions).
- **Vertex, pixel, geometry**: the stage cannot control which invocations share a subgroup, so an analysis proves,
  per operation, whether a cross-lane op can cross the 32-lane boundary. Quad operations and wave-uniform values
  never do. Each shader gets an **exactness label**: `exact` or `approximate(<reasons>)`.
- On 64-wide hardware (AMD) the lowering is skipped and wave64 runs natively.
- Exactness labels are stored and reported; approximate shaders are known, never hidden.

### 4.4 Storage

**`Output/leonida.db`** (a `redb` database: pure Rust, nothing to install) holds generated shaders and build
state. One write transaction per batch of shaders: a crash or Ctrl-C loses nothing and leaves no partial files.

Table `shaders`, keyed by the FSSM shader's content hash (32 bytes), value a `postcard`-encoded, schema-versioned
record:

| Field | Meaning |
|---|---|
| `stage`, `wave_size` | from the container and FSSM header |
| `exactness` | `Exact` or `Approximate(reasons)` |
| `translator_version` | version of the translator that produced the record |
| `spirv` | zstd-compressed (`ruzstd`) SPIR-V binary, absent on failure |
| `interface` | `ShaderInterface` |
| `error` | the `TranslateError` text, absent on success |

Reports (counts by stage, exactness, error) are computed by `leonida shaders report` from this table.

- Incremental rebuild: retranslate records whose `translator_version` differs from the current build.
- **Overrides** (readable and editable): `Mods/<package>/Shaders/<hash>.spvasm`, resolved through the virtual file
  system (section 7) like any other package content. `leonida shaders edit <hash> [--package <name>]` writes the
  generated shader's text into that package (default `Mods/my-shaders/`, created with a manifest on first use). On load an override is assembled and
  validated; if it fails, the error is reported and the generated shader is used. `leonida shaders check` lists
  overrides whose source shader changed after a translator update.
- **Reading**: `leonida shaders text <hash>` prints SPIR-V assembly; `leonida shaders export [filters] --out <dir>`
  writes `.spv` + `.spvasm` + `.json` (interface) files for RenderDoc, `spirv-dis` or diffing.

### 4.5 Phase 1 exit criteria

- All 26,503 shaders on the developer's corpus translate and pass validation, with exactness labels recorded.
- The translator test suite (section 9) passes, including GPU differential tests on the NVIDIA card.
- Full-corpus translation time is measured and recorded as the baseline for later regressions.

## 5. Assets (Phase 2)

### 5.1 Reading in place

The game's archives **are** the storage; nothing is copied to run Leonida.

| Layer | Contents | Size |
|---|---|---|
| Game archives | Read in place, memory-mapped (`memmap2`) | 0 extra |
| Asset index (`leonida.db`) | One row per resource: archive, offset, size, type, name hash, recovered name, content hash | an estimated few hundred MB |
| Hot cache | GPU-ready forms of frequently used assets, zstd-compressed, size-capped, least-recently-used eviction | **only built if profiling shows BCPack decoding is a bottleneck** |

- The index is built once by scanning archives for validated plaintext resources.
- Textures are detiled from the console layout **on the GPU** (compute shader during upload); BCPack is decoded on
  all CPU cores straight into upload buffers.
- Identical resources in several archives are decoded once (content hash).
- Loads are asynchronous; the renderer never waits on disk.
- Known gap: some BC6H/BC7 textures need history tables upstream never recovered. They get a placeholder and a logged
  entry.

### 5.2 Export (optional, for modding and viewing)

| Data | Format | Notes |
|---|---|---|
| Models | glTF 2.0 binary (`.glb`) | meshes, materials, skeletons, skinning, animations; opens in Blender |
| Textures | DDS | keeps exact BCn data; opens in Paint.NET, GIMP, Photoshop, Visual Studio. `--png` adds previews |
| Metadata | JSON beside each model | materials (shader hashes, texture links, parameters), LODs |
| Audio | Ogg Opus (lossless repackage), WAV for PCM and for XMA2 | XMA2 decoded by our own Rust decoder (also needed by the runtime's audio shims) |

Export fills the folder tree a piece at a time: `leonida assets export Characters/Lucia`. A full export is possible
but large (hundreds of GB).

## 6. Systems and config (Phase 2)

```
Systems/
├── Vehicles/    handling.xml, vehicles.xml, colours.xml, spawn rules
├── Weapons/     weapons.xml
├── Peds/        ped types, catalogues, relationships
├── World/       timecycle.xml (weather, time of day), population zones, fog, lights
├── Audio/       radio stations, music config, sound sets
├── Missions/    mission tables, story index (scripts listed, not decompiled)
├── Text/        <language>.json: every game string by key
├── Navigation/  navmeshes, plus a .glb preview
└── Camera/      camera settings
Config/          leonida.toml, graphics.toml, input.toml, mods.toml   (TOML)
```

- Data stored in the game as binary XML (upstream's `rbf0_*` tools) is exported as XML with exact structure, and
  **converted back** to binary when a package overrides it, so behaviour can be changed with a text editor.
- Leonida-derived tables (text, mission tables) are JSON.
- Categories follow what upstream has measured. A system whose format is not yet decoded contains a short note saying
  what is missing, not empty files.
- Compiled mission scripts are only listed; a script disassembler is a later, separate piece.

## 7. Virtual file system and packages (Phase 2)

Modelled on Microsoft Flight Simulator's package system.

- **Packages**: a folder with `manifest.json`:
  `{ "title", "author", "version", "type", "dependencies": [...], "game_version": "1.0.2.1" }`.
  Types: `assets`, `systems`, `shaders`, `patch` (upstream's JSON patch mods, e.g. first-person camera), `mixed`.
- **One virtual tree** built from layers, lowest priority first: base game (archives via the index, read-only) →
  Leonida's own fixes package → `Mods/` packages in the order given by `Config/mods.toml`, defaulting by type then
  name.
- Every consumer (renderer, tools, exporter, shims) reads through the same tree using readable paths such as
  `Characters/Lucia/Lucia.json` or `Systems/Vehicles/handling.xml`.
- **Better than MSFS**: no hand-maintained `layout.json` (Leonida scans and caches each package, refreshing on
  change); explicit load order; `leonida mods conflicts` lists every contested path and its winner; dependencies
  and game version are checked at startup with clear messages.
- **Optional, late**: `leonida mount` exposes the tree as a real folder in Explorer through Windows Projected File
  System; files are decoded on first open.
- **Limit**: the running game reads its own archives, whose tables of contents are encrypted, so archives cannot be
  rebuilt with mod content. Whole files opened by path (configs, loose files, saves) come from the tree through the
  file shim immediately; resource-level overrides for the running game need the game's resource loader hooked, which
  needs the executable (Phase 4+). The renderer harness and tools see all mods immediately.

## 8. Renderer (Phase 3)

- **`gpu`**: Vulkan 1.3 via `ash`, memory via `gpu-allocator`. Required on the development GPU: mesh shaders
  (`VK_EXT_mesh_shader`), fragment shader barycentrics, descriptor indexing (bindless), buffer device address.
  Ray tracing later. Validation layers on in debug builds.
- **Pipeline cache**: persistent `VkPipelineCache`; `leonida pipelines` builds every pipeline ahead of time.
- **`scene` harness**, in verifiable steps:
  1. A model drawn with our own simple shader (proves the asset readers).
  2. The game's vertex shader with our pixel shader.
  3. The game's vertex and pixel shaders with real textures.
  4. The game's deferred passes: G-buffer, then lighting.
- **Upgrades now**: precompiled pipelines, internal resolution scaling with FSR 1 (MIT, ported to WGSL) as upscaler, texture
  filtering overrides.
- **Upgrades that need the game running**: uncapped frame rate, ultrawide (game logic, camera and HUD are involved;
  done through code hooks).

## 9. Loader, shims and Xbox D3D12 (Phase 4)

Windows-only. Tested on synthetic executables until a game executable exists.

### 9.1 `pe-loader`

Order of operations, all driven by the game executable's own PE headers:

1. Map sections at the preferred base, or anywhere and apply base relocations (DIR64).
2. **Imports**: walk `IMAGE_DIRECTORY_ENTRY_IMPORT`. Each descriptor names a DLL and points at two parallel arrays:
   the function names/ordinals and the **IAT**. Resolve each entry (shim → allowed host DLL → blocking stub) and write
   the address into its IAT slot. Same for `IMAGE_DIRECTORY_ENTRY_DELAY_IMPORT`, bound eagerly.
3. Thread-local storage: allocate the TLS index and template, run TLS callbacks. (Upstream borrows OS TLS indices by
   loading copies of a helper DLL; we keep that mechanism but place the copies in `Output/runtime/`, not `%TEMP%`,
   and open them without delete sharing.)
4. Register exception tables (`.pdata`) with `RtlAddFunctionTable`.
5. Set final page protections. **Never writable and executable at once**: write, then switch to read-execute. Every
   `VirtualProtect` result is checked.
6. Jump to the **entry point**: image base + `OptionalHeader.AddressOfEntryPoint` (the C runtime's startup, not
   `main`).

Further rules:

- Import names are validated; none may resolve to a path outside the package.
- Code patches walk real instruction boundaries with `iced-x86` from the exception-table function ranges, never by
  scanning raw bytes for opcodes.
- Imports resolved at run time (`LoadLibrary`/`GetProcAddress`) are caught because those functions are shimmed.

### 9.2 `shims`

- Each shim is a Rust function with an attribute, e.g. `#[shim("xmem.dll", "XMemAlloc")]`; the table is collected at
  compile time.
- Written against the publicly documented GDK signatures. Before an executable exists, coverage is driven by the
  import tables of the package's own DLLs (`XboxBvhBuilder.dll`, `XCurl.dll`, `libHttpClient.GDK.dll`, ...).
- **Offline, no exceptions**: host DLLs on an allow-list (`kernel32`, `ntdll`, `user32`, ...). Every import,
  `LoadLibrary` and `GetProcAddress` that the `offline` crate names is bound to a stub that logs and fails as if
  there were no network: sockets, WinHTTP, WinINet, `libHttpClient.GDK`, `XCurl`, name lookups, network status,
  remote procedure calls, and the Xbox Live and `XNetworking` functions. No network function is ever made to work,
  not even through a shim, so the crash reporter's uploads, telemetry, Xbox Live and Rockstar services all fail
  quietly. The package's own network DLLs are never loaded. `leonida exe network` lists everything the policy
  catches in the executable.
- All file access goes through `vfs`: package read-only, packages layered on top, all writes to `Output/`. Paths the
  `offline` crate names are refused: `tcpip:` (the engine's remote file loading, which can also open a listening
  port), shared folders on other computers, and web addresses.

### 9.3 `xbox-d3d12`

- Implements the console D3D12 interface directly on `render` (no intermediate PC D3D12 device).
- Method tables are data: `d3d12_slots.toml` maps slot → method. Only about 9 slots are confirmed today; the rest
  assume PC order until the executable confirms them. Unknown slots trap and report slot number and caller address.
- Command lists are recorded and replayed on Vulkan; descriptor heaps map to bindless descriptors; barriers map to
  Vulkan synchronization2.
- Pipeline creation hashes shader bytes and looks them up in the shader store, overrides first.

### 9.4 First-boot kit

- `leonida run --dry-run <exe>`: reads the import tables without running anything; prints every import as shimmed,
  host, blocked or missing.
- `leonida run --trace`: writes to `Output/boot/` every import hit, every unknown D3D12 slot, every file accessed,
  and the last address reached.

## 10. Testing

- **Translator**:
  1. CPU reference (`testkit`) vs our SPIR-V on the GPU, compared lane by lane. Inputs are hand-written RDNA2
     assembly snippets assembled by `rdna`.
  2. Upstream's measured expectations (values measured on real AMD and NVIDIA hardware) ported as data, not code.
  3. Corpus verification on the developer's own shaders: `leonida shaders --verify` records translated / valid /
     exact counts in `leonida.db`; any drop is a regression. These numbers stay local.
- **Decoder**: decode → encode → decode round trips (`proptest`); cross-check against `llvm-mc` on random words
  (developers and CI only; never a user dependency).
- **Loader, archives, systems**: synthetic inputs built by test code; property-based fuzzing with `proptest`
  (`cargo-fuzz` is not used because it compiles C++ libFuzzer).
- **Renderer**: validation layers in debug builds; reference-image tests on synthetic scenes with a tolerance.
- Tests needing game files read `LEONIDA_GAME_DIR` and skip when it is unset.
- CI: `cargo fmt --check`, `clippy -D warnings`, tests on Windows and Linux, content scan for game data (magic
  bytes, not just extensions). GPU tests run locally via `cargo xtask gpu-test`.

## 11. Errors, logging, user experience

- Typed errors in libraries; the CLI prints plain messages with a hint (e.g. which driver feature is missing).
- Per-item failures (shader, asset) are recorded and summarized at the end; runs continue.
- Structured logs (`tracing`) to `Output/logs/`; runtime crashes write a minidump plus the boot trace to
  `Output/crash_reports/`.
- `leonida` with no arguments runs guided setup: find the package, check its version against the hash manifest
  (ported from upstream's `game/xbox-1.0.2.1.json`, hashes only), check GPU features and free disk space, build.
  Progress UI with `ratatui`.
- Subcommands: `check`, `shaders`, `assets`, `mods`, `mount`, `pipelines`, `view`, `run`.
- Releases: `leonida.exe` is published as a prebuilt binary (it contains no game data). Game-derived output is always
  built locally.

## 12. Phases

| Phase | Crates | Exit criteria | Estimated size |
|---|---|---|---|
| 0. Skeleton | workspace, `leonida` (`check`), CI | game found and version-checked; CI green | 2–4k |
| 1. Translator | `rdna`, `shader-container`, `shader-ir`, `shader-translate`, `shader-store`, `testkit` | section 4.5 | 55–70k |
| 2. Assets, systems, VFS | `rpf9`, `assets`, `systems`, `vfs` | full index built; one character exports to glTF + DDS and opens in Blender; measured systems round-trip XML ↔ binary | 15–25k |
| 3. Renderer | `gpu`, `render`, `scene` | harness steps 1–4; `leonida pipelines` builds every pipeline | 25–38k |
| 4. Runtime | `pe-loader`, `shims`, `xbox-d3d12` | synthetic executables load and run through shims; `--dry-run` works on the package DLLs; first-boot kit complete | 50–70k |

Total: roughly 150–200k lines of Rust plus a similar amount of tests. Each phase gets its own implementation plan.

## 13. Risks and open questions

1. **Constant-buffer layouts for the harness** (highest risk). Game shaders read constants the engine normally fills.
   Mitigation: check containers for reflection data first; otherwise infer layouts from how each shader uses each
   offset.
2. **Wave64 exactness in graphics stages on NVIDIA** may leave some shaders approximate. Mitigation: per-operation
   analysis and visible labels.
3. **BCPack BC6H/BC7 history tables** are not recovered; affected textures use placeholders.
4. **Xbox D3D12 method order** is mostly unconfirmed; data-driven slot table plus trapping keeps changes cheap.
5. **Name recovery coverage** decides how much of the readable tree is named rather than `_unnamed/`.
6. **Resource-level mod overrides in the running game** need the executable's resource loader measured.
7. **XMA2 audio** needs our own Rust decoder (no pure-Rust one exists); until it lands, XMA2 banks export without
   decoding and the runtime plays silence for them.
8. **Missing decoder source**: upstream's archive decoding lives in its `rpf9-explorer` submodule, absent from the
   reviewed zip. Readers are written from upstream's `docs/`; the submodule's own license must be checked before
   consulting its code.
9. **No game executable**: Phase 4 can only be verified on synthetic programs until one is available.
