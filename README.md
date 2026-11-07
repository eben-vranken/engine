# engine

*A small Rust 3D engine and editor for quiet, atmospheric, audio-first games.*

Crisp HD output. Very low-poly worlds. Strong light and real volumetric fog. Sound physically simulated from the geometry of the scene.

## Demo

https://github.com/user-attachments/assets/07985f75-9701-407d-8a18-6c98663c12f8

## What it is

A custom engine on standard Rust crates (wgpu, winit, egui, rapier3d, Steam Audio through AudioNimbus), built for a specific kind of game: slow, exploratory, first-person, where light and sound carry the mood. It is not built on Bevy or Godot, and it stays small on purpose: features are added when a game needs them.

A few ideas shape everything:

- **The editor is the game with a UI on top.** One binary, one runtime. Play runs the scene in place in the editor; Stop puts back the edited state. There is no export step.
- **Games are data, not code.** A game is a content-only project folder (settings, scenes, models, textures, audio) that lives outside this repository. Levels are built and behaviors composed in the editor.
- **One set of geometry serves everything.** The same low-poly shapes are drawn, collided with, baked into lightmaps and fed to the acoustic simulation.
- **Audio is first-class.** Walls muffle, doorways open up, marble halls ring and wooden rooms stay dry, without placing a single reverb zone by hand.

**Working name.** Built with [Claude Code](https://claude.com/claude-code).

## Rendering

The scene renders at a reduced internal resolution and is upscaled with sharp nearest-neighbor pixels. At the default it reads as HD with a faint pixel texture; the GPU time this saves goes into light and fog.

- **Render scale:** internal resolution = output ÷ an integer factor, so pixels stay perfectly square. AUTO picks about 540–720 internal lines (4K → ×3); a project can fix its own factor (×1–×8), and `[` `]` change it live.
- **Real-time direct light:** the sun with three shadow cascades, and up to 32 point lights (`LightSource`: color, intensity, range, flicker) with cached cube shadow maps for the nearest ones (16 on HIGH, 8 on LOW). Moving objects and flickering lanterns cast correct shadows.
- **Baked bounce light:** BAKE LIGHTING path-traces sky light, emissive surfaces and bounced light on all CPU cores into per-zone lightmaps. Lightmap UVs are generated automatically; moving objects get the same light from a grid of light probes. The bake is one `.lightmap` file next to the scene.
- **Materials:** primitives take a project texture (point-sampled, meters per repeat), roughness and metalness; glTF models bring their base color, normal, metallic-roughness and occlusion maps. Glossy surfaces get highlights from every light.
- **Reflections:** the light bake also captures one reflection cube per zone, box-projected to the zone's volume, so a polished floor reflects the window and the lantern in the right places.
- **Volumetric fog:** a camera-aligned 3D grid (froxels) lit through the shadow maps: light shafts through windows, halos around lanterns. Density, color and height falloff per zone, local `FogVolume` boxes, optional drifting wisps and lit dust motes, all smoothed over frames with temporal accumulation.
- **Post-processing:** emissive glow and bloom, exposure, AgX tone mapping, color grading per zone (exposure, contrast, saturation, tint, or a `.cube` LUT) and subtle film grain.
- **Performance:** two quality presets (`--quality high|low`). The target is 60+ fps at 4K output on an RTX 3060 Ti class GPU; the lighting test hall runs at about 85 fps (HIGH) on an Intel Iris Xe laptop at 1080p. `--perf-log` prints FPS and per-stage GPU times.

## Sound

Acoustics come from [Steam Audio](https://valvesoftware.github.io/steam-audio/) through [AudioNimbus](https://github.com/MaxenceMaire/audionimbus). The engine wraps it; it doesn't reimplement propagation.

- **HRTF** binaural positioning for headphones; the listener is the active camera, so the editor previews the acoustics while you fly through a space.
- **Occlusion and transmission:** solid objects are acoustic geometry. Sound behind a wall is muffled; opening a door lets the next room in. Doors and moving objects update the acoustic scene as they move.
- **Acoustic materials:** `AcousticMaterial` presets (wood, stone, marble, brick, concrete, plaster, carpet, velvet, glass, metal, water, gravel…) decide how surfaces absorb, scatter and transmit.
- **Reflections and reverb:** convolution reverb simulated live on a background thread, crossfaded as results arrive. BAKE ACOUSTICS places reverb probes on the floors of each zone and bakes them into a `.reverb` file.
- **Stylized reverb** *(being tested)*: an optional non-physical tail per zone, so an open void can still ring.
- **Air absorption**, a **simulation budget** (only sounds in the current and neighbouring zones are simulated; far ones fade out and keep their place) and a **master limiter**.
- **Footsteps** follow the head bob and depend on the floor's material, or play the game's own clips in order or at random.
- **Sound sources:** `SoundSource` plays WAV clips (looping or not, with volume) at an object.

## World

- **Scenes** are RON files, readable and diffable in git. **Models** are glTF (`.glb` preferred).
- **Zones:** every object belongs to a zone (a room, a hall, a courtyard). `ZoneVolume` boxes give a zone its extent; `Portal`s in doorways connect zones. Zones drive which zone you're in, frustum culling, the simulation radius (behaviors, physics and sound pause outside the current zone and its neighbours), light and reverb bakes, fog and color grading.
- **Physics** is rapier3d: static colliders for the level, dynamic bodies for props. Collision can be turned off per object (or per model part) to walk through it.
- **Behaviors** are plain Rust structs attached in the editor and stored as data:

| Component | What it does |
|---|---|
| `Primitive` | Box, plane, wedge, cylinder or sphere; its type can be changed later |
| `Model` | A glTF model, with optional per-part color, glow, collision and light |
| `Material` | Texture, scale, roughness and metalness |
| `PlayerStart` | Where the player spawns |
| `Floating` | Bobbing platforms and rocks |
| `Door` | Slides and/or swings open; acts as a sound portal |
| `Switch` | Press E or stand on it (pressure plate) to toggle a target, e.g. a door |
| `PhysicsBody` | Gravity and pushable props |
| `LightSource` | Colored, flickering, shadowed lights |
| `FogVolume` | Local fog beyond the zone default |
| `SoundSource` | Bells, drips, ambience, music |
| `AcousticMaterial` | How a surface sounds |
| `ZoneVolume`, `Portal` | Zone extent and connections |

## Player

A first-person kinematic controller with the polish going into feel: walk, sprint and crouch; ground and air acceleration; a jump defined by height and time-to-apex, separate fall gravity and variable jump height; coyote time and jump buffering; step climbing, slope limits and ground snapping; mouse look with FOV, head bob, landing dip and sprint FOV kick.

Every value lives in the game's `player.ron` (meters, seconds, degrees). Tune it live while playing with the F1 panel, or in the editor's inspector by selecting the Player Start; editing the file by hand hot-reloads it.

## Editor

Themed as a sparse amber-on-black terminal. The 3D viewport is drawn by the game renderer and takes most of the screen.

- **Scene tabs:** several scenes open side by side, each with its own undo history, selection, camera and unsaved state.
- **Building:** add primitives from the ADD menu, drag `.glb` / `.gltf` files from the desktop or the MODELS list onto the viewport (copied into the project and placed on the surface under the cursor), duplicate and delete. Re-exported models hot-reload.
- **Selecting and moving:** click or shift-click in the viewport or the hierarchy (grouped by zone, with search such as `candle > flame`), then move, rotate and scale everything selected with the gizmo, with snapping.
- **Inspector** generated from the components via `bevy_reflect`, never hand-written. Object references are picked from a list (a switch's target door). A change applies to every selected object as one undo step. Zones and the scene's environment (sun, sky, fog, grade) are edited the same way.
- **Undo/redo** for every edit.
- **Play/Stop in place** (F5).
- **Bakes:** BAKE LIGHTING and BAKE ACOUSTICS per scene.
- **Projects:** NEW / OPEN PROJECT, NEW SCENE, SAVE AS, PROJECT SETTINGS (name, start scene, render scale), and native file dialogs. `--launcher` opens a small window with recently edited projects.
- **Packages:** EXPORT PACKAGE writes a whole project to one `.gamepack` file to hand to someone else; opening it unpacks it and opens it in the editor exactly as it was.

### Modeler

A small modeler in its own tab for very low-poly props, with Blender-style controls. Not Blender: just enough to build and shape props cleanly.

- Start from a box, plane, cylinder or sphere; select vertices, edges or faces; move, rotate and scale with G/R/S, axis locks, typed amounts and snapping.
- Extrude, inset, delete, merge, weld, subdivide, loop cut, flip normals, mirror and randomize.
- Flat or smooth shading per face, vertex colors, one point-sampled texture with planar or box UV projection, roughness and metalness.
- **Parts:** a model can be made of named parts (a candle's stick, wax and flame), which a scene can give their own glow, collision and light.
- Saved as a standard `.glb` into the project's `models/`; scenes using it update live.

## Run

```
cargo run -p app                                    # play the sandbox
cargo run -p app -- --editor                        # edit the sandbox
cargo run -p app -- --launcher                      # pick a recent project, then edit it
cargo run -p app -- --editor --game ../my-game      # edit another game project
cargo run -p app -- --game my-game.gamepack         # unpack and open a project package
cargo run -p app -- --scene scenes/hall.ron         # play another scene of the project
```

Other flags: `--quality low` for integrated graphics, `--perf-log` to log FPS, GPU stage times and audio timing.

`sandbox/` is a small test project in this repo and the default `--game`. Its scenes: `test_level.ron` (movement test course), `acoustics.ron` (four spaces that sound different), `behaviors.ron` and `hall.ron` (the dark hall with one window and one lantern shown above).

**Building needs** a stable Rust toolchain and LLVM/Clang 9+ with `clang` on `PATH` (AudioNimbus generates bindings with bindgen; on Windows: `winget install LLVM.LLVM`). The first build downloads Steam Audio (about 180 MB) and can look stuck while it does. Developed on Windows (RTX 3060 Ti) and tested on Linux (Arch, KDE Wayland, Intel Iris Xe). On Linux, Steam Audio's `libphonon.so` from the build folder must be on `LD_LIBRARY_PATH`.

## Controls

| Where | Keys |
|---|---|
| Playing | WASD move, Space jump, Shift sprint, Ctrl/C crouch, E use, R respawn, F1 tuning panel, F2 fly camera, Esc free the mouse |
| Anywhere | `[` `]` render scale, 0 auto scale, V vsync |
| Editor | Click select, Shift+click add to selection, W/E/R move/rotate/scale, Ctrl snap, right mouse + WASD fly, Ctrl+D duplicate, Del delete, Ctrl+Z/Y undo/redo, Ctrl+S save, F5 play/stop, Tab/Shift+Tab panels |
| Modeler | 1/2/3 vertex/edge/face, G/R/S move/rotate/scale, X/Y/Z axis lock, E extrude, A select all, middle mouse orbit, Shift+middle pan, wheel zoom, numpad views |
| Audio A/B | F6 occlusion, F7 reflections, F8 HRTF, F9 air absorption, F10 baked/live reverb, Q clap, M move the looping sound to you |

## A game project

```
game.ron      name, start scene, render scale
player.ron    movement, camera and footstep feel
scenes/       RON scenes, plus their bakes (.lightmap, .reverb)
models/       glTF models
textures/     PNG / JPEG textures
audio/        WAV clips
```

The engine stays generic: anything specific to a game is data in its project folder, never engine code.

## Repo

```
BRIEF.md            what the engine is meant to be (scope, stack, principles)
docs/ROADMAP.md     milestones and where things stand
docs/DECISIONS.md   why things are the way they are
crates/engine/      the runtime library: render, lightmap, audio, scene, zones,
                    physics, player, behaviors, editor, modeler
crates/app/         the single binary: play mode, the editor, or the launcher
sandbox/            the test project
```

**Done:** renderer, first-person controller, editor, object creation, Play/Stop, behaviors, audio, zones, lighting, materials and reflections, scene tabs, project packages, the modeler, model parts, multi-select and project settings.
**Next:** stylized reverb (in testing), then abilities and gates, saving and a map generated from zones. See the [roadmap](docs/ROADMAP.md).

## License

MIT OR Apache-2.0.
