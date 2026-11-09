# Roadmap

Work through the milestones in order, one at a time. A milestone is done when its acceptance criteria are met **and the user has confirmed it** (seen, heard or played it). Tick items only then, and don't start the next milestone before that.

At the end of each milestone, tell the user exactly what to run and what to test.

## 0. Repo skeleton
- [x] Cargo workspace (`engine` lib, `app` bin) that builds and runs
- [x] BRIEF.md, CLAUDE.md, ROADMAP.md, DECISIONS.md
- [x] `--game <path>` argument; `sandbox/` as default project; game content lives outside this repo

## 1. Renderer basics
A window that draws simple 3D shapes the way the engine will look.

- [x] winit window + wgpu
- [x] Render at reduced internal resolution (output ÷ integer render-scale factor, default ~540–720 lines), nearest-neighbor upscale to the window; factor changeable live
- [x] Box and other primitive meshes generated in code (enough to build a grey-box level)
- [x] Simple lighting so shapes read: one directional light + ambient, basic shading
- [x] Point-sampled textures; a grid/checker texture for floors and walls (gives a sense of scale and speed)
- [x] Distance fog; tone mapping (AgX or ACES) from the start
- [x] Free-fly debug camera
- [x] egui overlay in the amber terminal style (BRIEF.md → UI style): FPS, internal and output resolution

**Acceptance:** boxes on a grid floor, crisp at any window size, render-scale changes live, 60+ fps.

## 2. First-person controller + test level
A first-person character in a 3D test level, with every movement property tunable live in-game.

- [x] Physics with `rapier3d`; static colliders for the level geometry
- [x] Test level built from boxes (defined in code or a simple RON file; it does not need to be the final scene format). It contains:
    - a large flat floor with the grid texture
    - ramps at 15°, 30°, 45° and 60°
    - stairs with different step heights (10, 20, 30, 40 cm)
    - gap jumps of increasing width and platforms of increasing height, color-coded so distances are recognizable
    - a narrow beam, a low-ceiling tunnel, a tall tower to fall from
    - walls and corners to run into and slide along
- [x] First-person controller (kinematic character controller):
    - walk, sprint, crouch
    - ground acceleration/deceleration, air acceleration and air control
    - jump defined by height and time-to-apex (gravity derived from them), separate fall gravity, max fall speed
    - variable jump height (releasing jump early cuts the jump)
    - coyote time and jump buffering
    - step climbing, max walkable slope, snap to ground
    - mouse look with sensitivity, FOV, optional head bob, landing dip, sprint FOV kick
- [x] All properties in one RON file in the game project: `sandbox/player.ron` (units: meters, seconds, degrees)
- [x] Tuning panel (F1): every property editable live while playing, with Save (writes `player.ron`) and Reset to defaults
- [x] Hot reload: editing `player.ron` by hand applies immediately
- [x] Debug readout: speed, grounded state, velocity, time since grounded
- [x] F2 switches between player and free-fly camera; R respawns at the start

**Acceptance (user plays it):**
- Walking, jumping and landing feel responsive; no snagging on steps or wall edges, no jitter.
- Changing a value in the panel is felt immediately, and saved values load on the next start.
- The user can tune the movement until it feels good.

## 3. Editor loop
- [x] `--editor` mode with egui, themed per the UI style in BRIEF.md
- [x] glTF loading
- [x] Scene format (RON, in the game project's `scenes/`); the test level becomes a scene
- [x] Click-to-select (raycast), transform gizmos, hierarchy grouped by zone
- [x] Inspector generated from `bevy_reflect` components
- [x] Undo/redo via commands
- [x] Save/load scenes

## 4. Creating objects
Add, remove and reshape objects from inside the editor. There are two ways to create objects:

- **Models from Blender (or similar):** drag a model file onto the viewport. glTF is the format, preferably the binary `.glb` (one self-contained file; in Blender: File → Export → glTF 2.0, format "glTF Binary"). `.gltf` works too. The file is copied into the game project's `models/` folder and the object is placed where it was dropped.
- **Primitives made in the editor:** an Add menu creates boxes, planes, wedges, cylinders and spheres. Because these shapes belong to the editor, their type can be changed afterwards in the inspector (box → cylinder and so on).

- [x] Add menu: create a primitive (box, plane, wedge, cylinder, sphere) in front of the camera, in the zone of the current selection
- [x] Change a primitive's shape type in the inspector, keeping sensible sizes
- [x] Drag and drop `.glb` / `.gltf` files onto the viewport: copied into `models/`, placed at the drop point on the surface under the cursor
- [x] Model textures: base color texture, point-sampled like the rest of the look
- [x] Delete and duplicate objects (Delete, Ctrl+D), undoable
- [x] Hot reload: re-exporting a model over its file in `models/` updates it in the editor

**Acceptance:** the user builds a small room from primitives and a model dragged in from Blender, reshapes a primitive, deletes and duplicates objects, undoes all of it, and saves.

## 4.1 Collision toggle
A small step right after milestone 4, for background and walk-through objects.

- [x] `collision` checkbox on primitives and models in the inspector, on by default, undoable
- [x] Off = the player passes through, but the object can still be clicked to select it (its collider moves to a group the player ignores)
- [ ] Later, if needed: a choice of full mesh / simple box / none, where "box" is a cheap collider for far-off detailed models

**Acceptance:** the user turns collision off on an object, walks through it in Play, and can still click to select it in the editor.

## 5. Play/Stop in place
- [x] Play runs the scene inside the editor; Stop restores the edited state
- [x] PlayerStart object; the player uses the game project's `player.ron`

## 6. First behaviors
- [x] Floating, Door, Switch, PhysicsBody
- [x] Object references in the inspector (switch → door)

## 7. Audio
Steam Audio in the engine, tested by walking through spaces in first person.

- [x] Add `audionimbus` (with `auto-install`) and an audio output crate (e.g. `cpal`)
- [x] Audio behaviors: SoundSource, AcousticMaterial (moved here from milestone 6, where they couldn't be heard)
- [x] Test scene with four connected spaces, each with its own acoustic material: a small wooden room (dry), a narrow stone corridor (flutter echoes), a large marble hall (long tail), an open area with no walls
- [x] Sources: a looping recording at `sandbox/audio/guitar_loop.wav` (fallback: generated tone), a clap on a key at the listener, footsteps depending on the floor material
- [x] A key moves the looping source into a different space
- [x] On-screen toggles for A/B comparison: occlusion, reflections/reverb, HRTF, air absorption
- [x] Simulation on a background thread; master limiter

**Acceptance (user walks through it with headphones):**
- The four spaces clearly sound different, sound is muffled behind walls and opens up through doorways, each toggle makes an audible difference, no clicks or clipping.

## 8. Zones
- [x] Zone data + connections in scenes
- [x] Frustum culling, simulation radius
- [x] Per-zone reverb probes

## 9. Lighting
- [x] Baked path-traced global illumination into lightmaps per zone ("bake lighting" button)
- [x] Volumetric fog at reduced resolution: light shafts, lantern halos, per-zone density/color/height; FogVolume behavior
- [x] Real-time shadows for dynamic lights and moving objects
- [x] Emissive surfaces lighting the bake and the fog
- [x] Post-processing: bloom, per-zone color grading (LUT), subtle grain
- [x] Temporal accumulation for live effects

**Acceptance:** 60+ fps at 4K output with default render scale on an RTX 3060 Ti class GPU; a dark grey-box hall with one window and one lantern looks atmospheric to the user.

## 10. Materials and reflections
Surfaces that read as stone, wood, plaster, wet floor or metal, and glossy ones that reflect the room around them.

- [x] `Material` component for primitives: a texture from the project (point-sampled, texels per meter), color, roughness; without one, the grid as now
- [x] Models use their glTF roughness and metalness
- [x] Highlights from the sun and lights on glossy surfaces (and none on rough ones)
- [x] Reflection probes baked per zone with BAKE LIGHTING, box-projected to the zone's volume, so glossy surfaces reflect the room; moving objects and live lights are not in them
- [ ] Optional, after the above: screen-space reflections for sharp, moving reflections in water and puddles

**Acceptance:** in the hall, a polished marble floor reflects the window and the lantern in the right places, a stone wall stays matte, and the frame rate on the laptop (Iris Xe) stays above 60 fps on HIGH.

## 10.1 Scene tabs
Several scenes open at once in the editor, one tab each.

- [x] A tab strip above the viewport: one tab per open scene, named after its file, marked when it has unsaved changes
- [x] `+` opens a scene from the project in a new tab (or switches to it if it's already open); the scene picker keeps replacing the current tab's scene
- [x] Each tab keeps its own undo history, selection, camera, bakes and unsaved state; switching back restores them
- [x] Closing a tab with unsaved changes asks SAVE / DISCARD / CANCEL; closing the window asks for every unsaved tab
- [x] Tabs can't be switched or closed while playing

**Acceptance:** the user opens the hall and the test level side by side, edits both, undoes in each separately, switches back and forth, and is asked before losing changes when closing a tab or the window.

## 10.2 Project package
One file that holds a whole game project for the editor, to hand to someone else. It is not a build of the game: it opens in the editor exactly as the project was.

- [x] A single-file package format for game projects: `game.ron`, `player.ron`, scenes, bakes, models, textures and audio, plus a small manifest (engine version, project name)
- [x] Editor: EXPORT PROJECT writes the open project to a package file
- [x] Opening a package (`--game <file>` or from the editor) unpacks it into a project folder and opens it in the editor
- [x] Engine-generated junk (caches, editor-only temp files) stays out of the package

**Acceptance:** the user exports the sandbox, copies the file to the laptop, opens it there, and the editor shows the same scenes, materials, sounds and bakes.

## 10.3 New scenes
Make scenes from the editor instead of writing RON files by hand.

- [x] NEW SCENE: name it, and it's created in the project's `scenes/` with a starting point (one zone, a floor, a Player Start, the default environment) and opened in a new tab
- [x] SAVE AS: save the current scene under a new name (a copy to branch from)
- [x] Names that are empty, taken or not valid file names are refused with a message

**Acceptance:** the user makes a new scene from the editor, builds in it, saves it, closes the editor, and finds it again in the scene picker and `+` menu next time.

## 10.3.1 Editor cleanup
The editor grew one feature at a time. Give it a proper menu bar, native file dialogs, new/open project, collapsible panels and a tidier codebase. No new features beyond this list.

- [x] Code split: `editor/mod.rs` and `runtime.rs` broken into modules named after what they do, with no behavior change
- [x] A menu bar (PROJECT, EDIT, ADD, BAKE, VIEW) with shortcuts shown next to items; every existing action reachable, nothing duplicated without reason
- [x] A slim toolbar row with only the constant controls: Play/Stop, W/E/R gizmo modes and snap hint
- [x] A status bar along the bottom: save/open/error messages and bake notes on the left, FPS, GPU, internal resolution and zone stats on the right; nothing dropped at 1600×900
- [x] Model tabs use the same menu bar, with scene-only actions (Play, bakes, ADD) hidden or disabled
- [x] Each side panel (and the modeler's left panel) collapses from a button in its own header to a thin strip with an expand button; Tab / Shift+Tab still work
- [x] Native file dialogs (rfd) that don't freeze the render loop, for opening projects and packages, exporting packages and SAVE SCENE AS
- [x] NEW PROJECT creates a content-only game folder (`game.ron`, `scenes/main.ron`, empty `models/`, `textures/`, `audio/`) and opens it; OPEN PROJECT refuses folders without `game.ron`
- [x] NEW PROJECT, OPEN PROJECT and OPEN PACKAGE ask about unsaved changes in every open tab first
- [x] Duplicated helpers merged, dead code and stale comments removed; consistent spacing, button sizes, headers, wording and hints across panels

**Acceptance:** at 1600×900 the user finds every action in the menus with its shortcut, sees messages and stats in the status bar without anything cut off, collapses and restores each panel with the mouse, creates a new project from the editor, opens another one and a package through the system file picker (on Windows and on the laptop), and notices no regressions in scene or model tabs.

## 10.4 Modeler
Removed in 10.9: models are made in Blender.

A small, simplified modeler inside the editor for very low-poly, PS1-style models. Not Blender: just enough to build and shape props and pieces cleanly, and drop them into a scene.

- [x] A model tab next to the scene tabs (`+` → NEW MODEL, or open a model from the project) with its own viewport, drawn by the same renderer
- [x] Start from a primitive (box, plane, cylinder, sphere) with a small number of segments
- [x] Select vertices, edges or faces (click, shift-click, box select); move, rotate and scale the selection with the gizmo, with snapping
- [x] Basic tools: extrude, inset, delete, merge vertices, split an edge / loop cut, flip normals, mirror
- [x] Flat or smooth shading per face; vertex colors
- [x] One texture per model from the project, point-sampled; simple UVs (planar / box projection per selection) and moving / scaling UVs
- [x] Every edit is an undo step
- [x] Save as glTF into the project's `models/`; scenes using the model update live (the existing hot reload)

**Acceptance:** the user builds a simple prop (a table or a lantern) from a box, colors and textures it, saves it, places it in the hall, then edits it again and sees the hall update.

## 10.5 Model parts
Models made of named parts (a candle's stick, wax and flame), each of which a scene can give its own look and light.

- [x] Modeler: a model has parts; + PART adds a box, plane, cylinder or sphere as a new part on top of the model; parts can be picked (selects their faces), renamed and deleted, and faces moved to another part; tools keep new faces in their part
- [x] Saved `.glb`s have one named node per part; any glTF's node (or mesh) names become its parts when loaded
- [x] Scene panel: a model with several parts opens to list them; picking one shows the part's settings in the inspector
- [x] A part can have its own color, emissive, collision and a light at its center; the rest of the model keeps the model's settings
- [x] Rendering, collision, light bakes and acoustics follow the part settings; scenes without part settings are unchanged

**Acceptance:** the user builds a candle (stick, wax, flame) as one model, places it in the hall, makes only the flame glow and gives it a flickering light, walks through the flame but not the stick, and bakes lighting.

## 10.6 Multi-select
Select several things at once and edit them together.

- [x] Shift-click in the scene panel adds or removes an object or a model part; shift-click in the viewport adds or removes an object; every selected object is highlighted
- [x] The inspector shows the last one picked; a change to it is copied to every other selected object (or part), only the field that changed, as one undo step; adding or removing a component, or giving parts their own settings, applies to all
- [x] The gizmo moves, turns and scales all selected objects together; EDIT ▸ DUPLICATE and DELETE act on all of them
- [x] Search `model > part` (e.g. `candle > sphere`) lists only that part of every matching model (`candle 3` too), opened; SELECT ALL picks every part found

**Acceptance:** the user opens several candles in the scene panel, shift-clicks every flame, gives them all a glow and a flickering light in one go, moves three candles together with the gizmo, and undoes each step.

## 10.7 Project settings
The game's own settings, edited from the editor instead of by hand in `game.ron`.

- [x] PROJECT ▸ PROJECT SETTINGS: the game's name, start scene and render scale (AUTO or a fixed ×1–×8)
- [x] The render scale previews live while the dialog is open; OK saves `game.ron`, CANCEL puts it back
- [x] The editor and the game start at the project's render scale; `[` / `]` still change it while running

**Acceptance:** the user sets their game to ×5, restarts the editor and plays, and both start at ×5 while the sandbox still starts on AUTO.

## 10.8 Stylized reverb
Reverb that doesn't come from geometry, so an open void or an empty plain can still ring (BRIEF.md: "Stylized reverb").

- [x] Per zone, optional: decay, brightness, level, and whether it replaces the simulated reverb; saved in the scene, edited under REVERB in the zone inspector
- [x] Every sound in the listener's zone gets the tail (Steam Audio parametric reverb); it fades over about a second when crossing into or out of the zone
- [x] F7 (reflections) switches it off along with the simulated reverb

**Acceptance:** in a zone with no walls, the user gives the zone a long dark tail, hears footsteps and claps ring out, walks out of the zone and hears it fade, and the void sounds the way they want.

## 10.9 Remove the modeler
Models come from Blender; the built-in modeler goes.

- [x] Modeler code, model tabs, NEW MODEL, double-click to open from the MODELS list and the orthographic camera removed
- [x] Model parts keep working: a glTF's named nodes are its parts, set up per scene as before
- [x] README updated

**Acceptance:** the editor opens and works as before without any modeler entry; models are still placed from the MODELS list and by dropping files, and a candle's parts can still be given their own glow and light.

## 11. Gameplay graphs
The game's flow and its dialogue as node graphs, edited in the editor (BRIEF.md: "Gameplay graphs"). Split into steps, each checked by the user before the next.

### 11.1 Graph editor
- [x] A graph tab next to the scene tabs: NEW GRAPH / open from `+`; saved as RON in the project's `graphs/`, with unsaved marks and asking before closing like scene tabs
- [x] Nodes on a canvas: add from a menu, drag, connect outputs to inputs, delete; pan and zoom
- [x] Node fields edited in the inspector beside the canvas, generated from `bevy_reflect` (node types are plain reflected types, like behaviors)
- [x] Every edit is an undo step
- [x] A first node set to lay out a story: Start, a note/comment, and the events and actions of 11.2 as placeholders that already save and load

**Acceptance:** the user builds a small flow graph (start → enters the hall → door opens), saves it, closes the editor, and finds it again exactly as it was.

### 11.2 Flags, events and actions
- [x] Flags (yes/no, number, text) declared in the project; a flags panel
- [x] Events: game start, scene loaded, enter/exit a zone or TriggerZone, a switch used, an object picked up, a dialogue ended
- [x] Actions: set a flag, open/close a door, show/hide an object, play a sound, load a scene, start a dialogue
- [x] Conditions on connections, from flags
- [x] The game runs the project's flow graph; several nodes can be active at once
- [x] During Play in the editor, active nodes are highlighted and flags shown live

**Acceptance:** in the sandbox, walking into a room opens a door and plays a sound only after a switch was used, and the user watches the graph step along while playing.

### 11.3 Dialogue
- [x] Dialogue graphs: a Line node (speaker, text, optional sound), replies (text, condition, actions), and end
- [x] Character behavior: talked to with E; starts its dialogue graph
- [x] Speaker names are state: a Rename action changes what a character is called from then on (`???` → their name)
- [x] Presentation set in the game project: text box or subtitles, position, colors, font, typing speed; default an amber text box
- [x] Lines play their sound if they have one; silent lines are fine

**Acceptance:** the user talks to an NPC named `???`, picks between replies that lead to different answers and set different flags, a reply opens a door, and a later conversation shows the NPC's real name.

### 11.4 Saving
- [x] SavePoint behavior; save and load of flags, active graph nodes, player position and per-zone object state
- [x] Loading restores the game exactly where it was saved

**Acceptance:** the user saves mid-story, quits, loads, and the world, the flags and the next dialogue are as they left them.

### 11.5 Abilities and gates
- [x] Abilities are flags; AbilityPickup sets one, AbilityGate blocks the way without it
- [x] Controller abilities read from flags (the first one a game needs, e.g. double jump)

**Acceptance:** a gate the user can't pass opens after picking up the ability, and the ability survives a save and load.

### 11.6 Map
- [x] A map generated from zone data, revealing zones as they're visited; visited zones are saved

**Acceptance:** the user opens the map, sees only the zones they've been to, and it survives a save and load.

## 11.7 Graph owners and scene graphs
- [x] Every graph shows its owner (the game's flow, a scene, a character, or none) and what uses it, with links there
- [x] Each scene can have a graph of its own (`graphs/scenes/<scene>.ron`): it starts when the scene loads, pauses when the player leaves and carries on where it was when they return; saved with the game
- [x] The inspector lists the graphs that mention the selected object; references that point at nothing are flagged under the canvas

**Acceptance:** the user adds a graph to a scene from LOGIC, sees who owns each graph and what uses it, and a scene graph picks up where it was after leaving and coming back.

## 11.8 Editor redesign
The editor reorganised for ease of use; same look, same features (design: the "Editor UX Redesign" canvas).

- [x] Workspaces PROJECT / BUILD / LOGIC in the top bar with a breadcrumb; menus FILE, EDIT, ADD, VIEW; CTRL+K command palette; `?` shortcut list
- [x] PROJECT page: start screen (recent, new, open, packages) and every project-wide setting (name, startup, rendering, player feel, dialogue look with live preview, flags, saves)
- [x] BUILD: outliner with type tags and filters, assets drawer (models, textures, sounds), floating viewport tools with SHOW and bake state, trigger boxes in the viewport, inspector cards, grouped ADD BEHAVIOR
- [x] LOGIC: graphs grouped by owner with flags, canvas with category tags, FIND and an overview, CONVERSATION view with preview and TEST CONVERSATION
- [x] PLAY: playing bar with STOP and RESTART, test drawer (logic and events, flags, audio, feel, map, stats), map on M
- [x] Secondary text readable (6.3:1); lines in their own darker color

**Acceptance:** the user finds every former feature in its new place, works a session in each workspace, and prefers it to the old layout.

## 11.9 One test level
- [x] One `test_level.ron` with every feature once, around a plaza: course (north), acoustics wing (east), dark hall (west), yard with story, abilities, saving and behaviors (south), void with stylized reverb (south-east)

**Acceptance:** after baking once, the user tests every feature in one scene without duplicates.

## 11.10 Scene graphs only, and scripts
- [ ] No game-wide flow graph: each scene's own graph holds its logic; only the loaded scene's graph (and what it started) stays in memory. Flags stay global
- [ ] LOGIC opens the open scene's graph first
- [ ] Scripts (Rhai) in the project's `scripts/`: `start()` once and `update(player, dt)` every frame while on; player state, velocity, jump, keys, flags; reload on save; errors shown, never crash
- [ ] ABILITY node turns a script on or off; abilities that are on are saved with the game
- [ ] The controller's built-in double jump is gone; the sandbox's is `scripts/double_jump.rhai`, turned on by the feather
- [ ] Script behavior: any object runs a script (`start(object)`, `update(object, dt)`): position, rotation, scale, visible; `player()` from any script. The sandbox's watcher turns to face the player
- [ ] Script tabs in LOGIC: + SCRIPT / NEW SCRIPT from a template, colored code with line numbers, live syntax check with the line, formatting on save (Ctrl+S) or FORMAT, what uses the script and a reference of what it can call
- [ ] LOGIC's left panel: graphs, scripts and flags stacked, no longer drawn over each other

**Acceptance:** the user plays the test level, can't double jump until picking up the feather, then can (and still can after a save and load); changes the script while playing and feels the change; and the gate story still works from the scene graph alone. They make a script of their own in the editor, saving formats it, they put it on an object and watch it run.

## 12. Stairs primitive
A small step after the core milestones: stairs as an editor primitive, shaped from the inspector.

- [ ] `Stairs` in the Add menu and the inspector's TYPE picker
- [ ] Inspector fields to shape the flight: width, total height, number of steps, step depth (step height follows from height ÷ steps)
- [ ] Curve: a turn angle over the whole flight (0° straight, 90° quarter turn, 360°+ spiral) and an inner radius (0 = around a central post)
- [ ] Collision per step, so the player's step climbing works on every shape

**Acceptance:** the user builds a straight flight, a curved flight and a spiral staircase from one primitive, and walks up each.

## Later (optional)
Not scheduled; picked up only if a game needs it.

- [ ] Music system (was milestone 10): one key, scale and tempo per game (overridable per zone), motifs quantized to the beat, instrument layers that change per zone, music as positioned sources heard through the acoustic simulation
- [ ] Singer behavior: characters that sing short motifs (voice, motif, interval, reacts_to_player). Needs the music system and a decision on voices (BRIEF.md open question); games can use recorded audio tracks through SoundSource meanwhile