# engine

*A small Rust 3D engine and editor for quiet, atmospheric, audio-first games.*

Crisp HD output. Very low-poly worlds. Strong light and real volumetric fog. Sound physically simulated from the geometry of the scene.

## Demo

https://github.com/eben-vranken/engine/blob/main/preview.mp4

*A dark grey-box hall. Sunlight cuts through a window as a shaft in the fog; a lantern glows at the far end.*

## About

**Working name.** Games made with the engine are content-only project folders that live outside this repository. The editor is the game with a UI on top.

Built with [Claude Code](https://claude.com/claude-code).

## What it does

**Look**
- Renders at a reduced internal resolution (about 540–720 lines) and upscales with sharp nearest-neighbor pixels; the saved GPU time goes into light and fog.
- Real-time shadows from the sun (three cascades) and from point lights (cached cube maps), so moving objects and flickering lanterns cast correct shadows.
- Volumetric fog in a camera-aligned 3D grid, lit through the shadow maps: light shafts through windows, halos around lanterns, fog per zone and local `FogVolume`s, smoothed over frames (temporal accumulation).
- Baked bounce light: **BAKE LIGHTING** path-traces sky, emissive and bounced light into per-zone lightmaps (UVs generated automatically), plus light probes for moving objects.
- Emissive surfaces, bloom, AgX tone mapping, color grading per zone (with `.cube` LUT support) and subtle film grain.

**Sound** (Steam Audio, via AudioNimbus)
- HRTF, occlusion and transmission through walls and doors, per-surface acoustic materials.
- Reflections and reverb simulated on a background thread, plus reverb baked per zone (**BAKE ACOUSTICS**).

**World and editor**
- Scenes are RON files; models are glTF (drag and drop onto the viewport, hot-reloaded on re-export).
- Every object belongs to a zone. Zone volumes and portals drive culling, which zone you're in, the simulation radius, bakes, fog and grading.
- Inspector generated from the components, undo/redo for every edit, Play/Stop in place.
- Behaviors composed from data: `Floating`, `Door`, `Switch`, `PhysicsBody`, `SoundSource`, `AcousticMaterial`, `LightSource`, `FogVolume`.
- A tunable first-person controller (rapier3d); every movement value lives in the game's `player.ron` and can be tuned live (F1).

## Run

```
cargo run -p app                                    # play the sandbox
cargo run -p app -- --editor                        # edit the sandbox
cargo run -p app -- --scene scenes/hall.ron         # play another scene of the project
cargo run -p app -- --editor --game ../my-game      # edit another game project
```

`sandbox/` is a small test project in this repo and the default `--game`. `scenes/hall.ron` is the lighting test scene shown above.

**Building needs:** a stable Rust toolchain and LLVM/Clang 9+ with `clang` on `PATH` (AudioNimbus generates bindings with bindgen). The first build downloads Steam Audio (about 180 MB) and can look stuck while it does. Developed on Windows with an RTX 3060 Ti; the target is 60+ fps at 4K output.

## Controls

| Where | Keys |
|---|---|
| Playing | WASD move, Space jump, Shift sprint, Ctrl/C crouch, E use, R respawn, F1 tuning panel, F2 fly camera, Esc free the mouse, `[` `]` render scale, V vsync |
| Editor | Click select, W/E/R move/rotate/scale, Ctrl snap, right mouse + WASD fly, Ctrl+D duplicate, Del delete, Ctrl+Z/Y undo/redo, Ctrl+S save, F5 play/stop |
| Audio A/B | F6 occlusion, F7 reflections, F8 HRTF, F9 air absorption, F10 baked/live reverb, Q clap, M move the looping sound to you |

In the editor, click a zone's header in the hierarchy to edit its fog and color grade.

## A game project

```
game.ron      name and start scene
player.ron    movement and camera feel
scenes/       RON scenes, plus their bakes (.lightmap, .reverb)
models/       glTF models
audio/        sound clips
```

The engine stays generic: anything specific to a game is data in its project folder, never engine code.

## Repo

```
BRIEF.md            what the engine is meant to be (scope, stack, principles)
docs/ROADMAP.md     milestones and where things stand
docs/DECISIONS.md   why things are the way they are
crates/engine/      the runtime library: render, lightmap, audio, scene, zones, behaviors, editor
crates/app/         the single binary: play mode, or the editor with --editor
sandbox/            the test project
```

Done so far: renderer, first-person controller, editor, behaviors, audio, zones, lighting (milestones 0–9). Next: abilities, saving and a map. See the roadmap.

## License

MIT OR Apache-2.0.
