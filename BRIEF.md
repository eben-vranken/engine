# Engine Brief

The source of truth for what this engine is. If code and brief disagree, ask before changing either.

## Purpose

A minimal 3D game engine and editor in Rust, built with Claude Code, for quiet, atmospheric, audio-first games with low-poly worlds and striking light. Games are made by building levels, composing behaviors and connecting gameplay graphs in the editor; small scripts add what those don't cover, such as the player's abilities.

- **Look:** crisp HD output of very low-poly worlds, with atmospheric lighting and real volumetric fog. Moody, not photorealistic.
- **Sound:** physically simulated acoustics (occlusion, reflections, reverb) as the engine's signature feature.
- **Workflow:** a built-in 3D editor where objects are selected, placed and given behaviors visually.
- **Growth:** start bare-bones and add features only when a game actually needs them.

## Core principles

1. **The editor is the game with a UI on top.** One binary, one runtime. Play runs the scene in place; Stop restores the edited state. No export step.
2. **Data-driven.** Scenes, behaviors and content live in readable files. Adding a new object type means writing one Rust struct; it appears in the editor automatically.
3. **Audio is first-class.** Geometry, materials and zones serve sound as much as visuals. One set of low-poly geometry feeds rendering, lighting and acoustics.
4. **Minimal first.** Build only what is needed now. Add an editor feature only after feeling its absence three times.
5. **Optimize when profiling says so**, but structure data (zones, connections) so later optimizations are easy.
6. **Engine and games are separate.** The engine is generic and open source. Each game is a content-only project folder (settings, scenes, models, audio) outside this repo, loaded with `--game <path>`. The `sandbox` project in this repo is used for testing.

## Tech stack

A custom engine on standard Rust crates, not built on an existing engine.

| Area | Choice | Notes |
| --- | --- | --- |
| Language | Rust (edition 2024) | Engine and editor in one workspace |
| Rendering | wgpu | Compute shaders for lighting effects; no RTX hardware required |
| Windowing and input | winit | |
| Editor UI | egui | Panels, inspector, menus; custom amber terminal theme |
| Gizmos | transform-gizmo-egui | Move, rotate, scale handles |
| Reflection | bevy_reflect (standalone) | Auto-generates inspector UI from structs |
| Scene format | RON | Plain text, diffable in git |
| Assets | glTF | Models made in Blender or similar |
| Physics | rapier3d | Colliders, kinematic character controller, rigid bodies |
| Acoustics | AudioNimbus (Steam Audio) | Occlusion, reflections, reverb, HRTF |
| Gameplay logic | Node graphs (RON) edited in the editor | Game flow and dialogue; see Gameplay graphs |
| Scripting | Rhai | Abilities and game-specific moves, turned on by graphs; see Scripts |

## Rendering

Crisp HD output, very low-poly geometry, and lighting as the priority. The scene renders at a reduced internal resolution and is upscaled with sharp nearest-neighbor pixels; the compute this saves goes into lighting and fog.

**Resolution**

- Internal resolution = output resolution divided by an integer render-scale factor, so pixels stay perfectly sharp.
- Default: the factor that puts the internal height around 540–720 lines. 1080p → 960×540 (2×), 1440p → 1280×720 (2×), 4K → 1280×720 (3×).
- Pixels are barely visible at the default: it reads as HD with a subtle texture. The factor is a setting: 1 = native, higher = chunkier.
- Displays that don't divide evenly use the nearest factor and letterbox.

**Look**

- Very low-poly geometry with simple shading.
- Low-res, point-sampled textures, so texels read as crisp and intentional at HD.
- Optional retro toggles per game: vertex snapping, affine texture warping, dithering. Off by default.

**Lighting and atmosphere**

- **Baked global illumination:** path-traced lightmaps per zone, via a "bake lighting" button in the editor. Bounce light, color bleeding and soft shadows from static lights; near-free at runtime.
- **Volumetric fog:** real participating media that lights scatter through: light shafts from windows, halos around lanterns. Density, color and height per zone. Computed at reduced resolution and upsampled.
- **Real-time shadows** for dynamic lights and moving objects.
- **Emissive surfaces** (braziers, candles, glowing water) light their surroundings, both in the bake and in fog.
- **Live ray-traced effects** in compute shaders with a custom BVH where they earn it; reduced resolution plus temporal accumulation keeps them cheap.
- **Post-processing:** tone mapping (AgX or ACES), bloom, per-zone color grading (LUTs), subtle film grain.
- Art direction matters more than compute: few lights, strong contrast, deep darkness.

**Performance targets**

- 60+ fps at 4K output with default render scale and all effects on a mid-range GPU (RTX 3060 Ti class).
- A low preset (higher render-scale factor, reduced fog quality) that stays at 60 fps on modest hardware.

## Audio

Sound propagation is simulated from scene geometry using Steam Audio through AudioNimbus; the engine wraps it, it does not reimplement acoustics.

- **Occlusion and transmission:** sound behind walls is muffled; opening a door lets the next room's sound in.
- **Reflections and reverb:** baked reverb at probe points per zone for static spaces, blended as the listener moves; live simulation for nearby, moving sources.
- **Threading:** reflection simulation runs on a background thread at its own rate so it never stalls rendering.
- **HRTF:** binaural 3D positioning for headphones.
- **Acoustic materials:** marble, wood, velvet, water, metal and so on, assigned on the same objects as visual materials.
- **Air absorption:** distant sounds lose high frequencies; in open spaces sounds carry far.
- **Stylized reverb:** optional non-physical reverb (for example an impossibly long, faint tail) per zone or per sound.
- **Footstep system:** surface material drives the footstep sound.
- **Simulation budget:** only sources in the current and neighbouring zones get full simulation; distant ones use simplified paths.
- **Master limiter** to keep loud events from clipping.
- **Editor preview:** hear the acoustics while flying the editor camera through a space.

## World structure and performance

Load the whole world at once, but organize it into zones from day one.

- **Zones:** every object belongs to a zone (a room, an island, a building). Doorways and passages are explicit connections between zones.
- Zones drive culling, simulation radius, per-zone light bakes, reverb probes, fog settings, the map, saving and ability gating.
- **From the start:** frustum culling, and a simulation radius so behaviors, physics and NPCs only update in the current and neighbouring zones.
- **Large worlds:** re-center the world on the player periodically (origin rebasing) to avoid floating-point jitter far from the origin.
- **Expected hotspots:** volumetric fog and live lighting effects (bounded by render scale), and acoustic simulation per active source. Not polygon count.

## Editor

A minimal in-engine 3D editor, grown feature by feature. External tools (Blender or similar) are for modelling; the editor has no modeler of its own. Level layout, behavior and gameplay graphs happen in the editor.

**Core features**

- 3D viewport with a fly camera, rendered by the game renderer.
- Click-to-select via a raycast against the scene BVH.
- Gizmos for move, rotate and scale; an object hierarchy grouped by zone.
- Inspector generated from reflected components: add, remove and edit behaviors with typed fields.
- Object references in the inspector (a switch's `target` points at a door).
- Undo/redo for every edit, built on a command system from the start.
- Save and load scenes as RON; hot reload of glTF and data files.
- Play/Stop in place.
- Graph tabs next to the scene tabs, where gameplay graphs are built from nodes and connections (see Gameplay graphs).

**Later, when needed**

- Box brushes for walls, floors and arches.
- Prefabs (reusable object templates).
- Acoustic preview and a "bake lighting" button.
- Live tweaking of values while the game is playing.

## UI style

One visual language for the editor and for games' default UI: minimal, almost terminal-like, in the spirit of Blade Runner and Evangelion interfaces but sparser.

- **Color:** a single amber-orange accent on near-black. A dimmer orange for secondary text and lines; one alert color used rarely.
- **Type:** one monospace typeface, small sizes, uppercase labels.
- **Shapes:** thin 1 px lines and frames, square corners, no shadows, gradients or rounded buttons. Text over icons wherever text works.
- **Density:** show only what the task needs. Panels collapse; empty space is fine.
- **Detail, used sparingly:** a blinking cursor, bracketed states like `[ACTIVE]`, optional faint scanlines or phosphor glow.
- **Editor:** egui fully re-themed to this style; the 3D viewport takes most of the screen.
- **In-game default:** almost no HUD. Text-only prompts that fade in and out; menus in the same terminal style.

## Behavior library

Reusable components written once in Rust and attached in the editor. This library is what makes games mostly no-code.

| Behavior | Example fields | Purpose |
| --- | --- | --- |
| Floating | height, speed, phase | Bobbing rocks and platforms |
| Rotating | axis, speed | Turning objects |
| Mover | path, speed, loop | Moving platforms |
| PhysicsBody | mass, starts_asleep, wakes_on | Gravity and dynamic props |
| Crumbling | delay, respawn | Platforms that fall when stepped on |
| Door | opens_with, swing or slide, speed | Doors that also act as sound portals |
| Switch / PressurePlate | target, mode | Triggers another object |
| TriggerZone | size | Fires enter / exit events that gameplay graphs react to |
| Character | id, name, talk sound | Someone who can be talked to; starts a dialogue graph. The shown name is game state and can be changed by graphs |
| AbilityGate | requires | Blocks the way unless a flag (an ability) is set |
| AbilityPickup | grants | Sets a flag (unlocks an ability) when picked up |
| SavePoint | | Saving and respawn |
| SoundSource | clip, loop, volume | Bells, drips, organs, ambience |
| LightSource | color, intensity, flicker, baked or live | Braziers, candles, lanterns |
| AcousticMaterial | preset or absorption and scattering | How surfaces sound |
| FogVolume | density, color, height falloff | Local fog beyond the zone default |

## Gameplay framework

Generic systems for 3D platformer and exploration games; the character controller deserves the most polish.

- **Character controller:** first-person, kinematic. Walk, sprint, crouch; acceleration and air control; jump defined by height and time-to-apex, separate fall gravity, variable jump height; coyote time and jump buffering; step climbing and slope limits. Abilities such as double jump or wall climb are scripts that plug into it; the controller has none built in.
- **Tunable from data:** every movement and camera property lives in the game project's `player.ron`, editable live in an in-game tuning panel and hot-reloaded from disk. Feel is tuned by playing, not by recompiling.
- **Camera:** first-person mouse look with FOV, sensitivity, optional head bob, landing dip and sprint FOV kick. A third-person camera can be added later if a game needs it.
- **Ability system:** an ability is a script, turned on or off by a graph's ABILITY node and saved with the game. Gates and pickups work with flags, which scripts and graphs share.
- **Save system:** player state, flags, abilities that are on, active graph nodes, and per-zone object state.
- **Map:** generated from zone data, revealing zones as they are visited.
- **Player sound actions:** an optional hum or clap input, routed through the acoustic simulation.

## Gameplay graphs

The game's logic is data: graphs of nodes and connections, built in a graph tab in the editor and saved as RON in the game project's `graphs/`. They replace code for everything from the overall story to a single conversation.

- **Two uses, one system.** A *scene graph* holds one scene's logic (once the player reaches the hall, this happens; then that). Each scene has at most one, and only the loaded scene's graph is in memory; it pauses when the player leaves and carries on when they return. A *dialogue graph* holds one conversation. Both use the same nodes, connections, conditions and actions; dialogue only adds its own node types. Scene graphs start dialogues; graphs can start other graphs. There is no game-wide graph: flags are global, so one scene's graph sets what another's checks.
- **Events, actions, conditions.** Events move a graph on: the game starts, a scene loads, the player enters a zone or trigger, uses a switch, picks something up, a dialogue ends. Actions change the world: open a door, show or hide an object, play a sound, load a scene, start a dialogue, set a flag, rename a character. Conditions guard connections ("only if the lantern was found").
- **Flags** are the game's named state (yes/no, numbers, text), set and checked by graphs. Abilities, dialogue outcomes and story progress are all flags, and saving is writing out flags, active nodes and object state.
- **Several nodes can be active at once**, so side threads run next to the main one.
- **Dialogue:** an NPC says a line, the player picks one of several replies, the NPC answers at once. Each line has a speaker, text and an optional sound (a voice noise, a tone, or nothing). Each reply can have a condition and actions. Speaker names are state: a character can be `???` until a graph reveals or changes their name.
- **Presentation is per game.** How dialogue is shown (a text box, subtitles, position, colors, font, typing speed) is configured in the game project. The engine's default is a text box in the amber UI style.
- **Built to grow.** Node types are plain Rust structs with `bevy_reflect`, like behaviors, so a new node is one struct and its fields appear in the graph editor automatically. Graphs start with story beats and must be able to carry more (puzzle logic, timers, counters) later without a new format.
- **Debugging:** during Play in the editor, the graph tab highlights the active nodes and shows the flags.
- Not a visual programming language: no per-frame logic, math or loops in graphs. Behaviors do the moment-to-moment work; graphs decide what happens next.

## Scripts

Small Rhai scripts in the game project's `scripts/` for what graphs and behaviors don't cover, first of all the player's abilities.

- A graph's ABILITY node turns a script on or off; while it's on the engine calls its `update(player, dt)` every frame after the controller moved (and `start()` once when it's turned on).
- Scripts read the controller's state and keys, change the player's velocity or make it jump, and read and set flags, so graphs and scripts talk through flags.
- Scripts reload when saved. An error stops the script (shown in the editor and the test drawer), never the game; a step limit stops endless loops.
- The API grows with what games need, like node types and behaviors.

## Milestones

See `docs/ROADMAP.md` for the checklist and acceptance criteria.

## Deferred until needed

- Portal culling (zone data makes this straightforward later).
- World streaming.
- LOD.
- Hardware ray tracing paths.
- Heavy denoising.
- A music system (shared key/scale/tempo, beat-quantized motifs, instrument layers per zone) and singing characters; games play their own recorded music through SoundSource.

## Open questions

- Final name for the engine (working name: `engine`).

## Sources

- AudioNimbus (Steam Audio for Rust): https://github.com/MaxenceMaire/audionimbus
- Steam Audio: https://valvesoftware.github.io/steam-audio/
