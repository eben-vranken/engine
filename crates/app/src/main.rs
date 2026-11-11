//! Single entry point. The editor is the game with a UI on top:
//!
//!   cargo run -p app                                -> play the sandbox
//!   cargo run -p app -- --editor                    -> edit the sandbox
//!   cargo run -p app -- --editor --game ../my-game  -> edit another game project
//!   cargo run -p app -- --launcher                  -> pick a project to edit
//!
//! `--launcher` opens a small window listing the projects edited before
//! (most recent first), plus OPEN PROJECT… and NEW PROJECT…; the one picked
//! opens in the editor.
//! `--game <path>` picks the game project folder (default: `sandbox`), or a
//! project package (`.gamepack`), which is unpacked next to it and opened.
//! `--scene <path>` opens another scene than the project's start scene
//! (relative to the project folder, e.g. `scenes/test_level.ron`).
//! `--quality low` uses the low preset (for integrated graphics);
//! `--perf-log` logs frame-time percentiles, long frames, hitch timers, FPS
//! and GPU times every two seconds (the same numbers are in F3 STATS).
//! `--new-game` plays from the start even if there is a saved game.
//! `--player` plays the game as a player gets it: no tuning panel, test
//! drawer, debug readout or debug keys (the editor's Shift+F5 opens this).
//! `--bench <route.ron>` walks the player along a route on its own, then
//! quits (always a new game); with `--perf-log`, for before/after numbers.
//! `sandbox/bench.ron` is the sandbox's route.
//! Game projects are content-only folders; they never contain engine code.
//!
//! `--export <file>` writes the game as an executable for players (this
//! executable with the project inside it) and quits; use a release build.
//! The editor's EXPORT GAME does the same. Started, the exported copy plays
//! the game as `--player` does and ignores `--editor`, `--game`, `--bench`
//! and `--export`.
//!
//! Release builds on Windows are window-only apps (no console window);
//! debug builds keep the console for the log.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use engine::project::Project;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Play,
    Editor,
    Launcher,
}

struct Args {
    mode: Mode,
    game: PathBuf,
    scene: Option<PathBuf>,
    quality: engine::render::quality::Quality,
    perf_log: bool,
    new_game: bool,
    player: bool,
    bench: Option<PathBuf>,
    export: Option<PathBuf>,
}

fn parse_args() -> Args {
    let mut mode = Mode::Play;
    let mut game = PathBuf::from("sandbox");
    let mut scene = None;
    let mut quality = Default::default();
    let mut perf_log = false;
    let mut new_game = false;
    let mut player = false;
    let mut bench = None;
    let mut export = None;
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--editor" => mode = Mode::Editor,
            "--launcher" => mode = Mode::Launcher,
            "--game" => {
                if let Some(path) = args.next() {
                    game = PathBuf::from(path);
                }
            }
            "--scene" => scene = args.next().map(PathBuf::from),
            "--quality" => match args.next().map(|q| q.parse()) {
                Some(Ok(q)) => quality = q,
                Some(Err(err)) => eprintln!("{err}"),
                None => eprintln!("--quality needs high or low"),
            },
            "--perf-log" => perf_log = true,
            "--new-game" => new_game = true,
            "--player" => player = true,
            "--bench" => bench = args.next().map(PathBuf::from),
            "--export" => export = args.next().map(PathBuf::from),
            other => eprintln!("ignoring unknown argument: {other}"),
        }
    }

    Args {
        mode,
        game,
        scene,
        quality,
        perf_log,
        new_game,
        player,
        bench,
        export,
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu_hal=error,wgpu_core=warn"),
    )
    .init();
    // An exported game: this executable with a project inside it.
    let embedded = engine::standalone::embedded_game();
    let standalone = !matches!(embedded, Ok(None));
    let result = embedded.map_err(Into::into).and_then(run);
    if standalone && let Err(err) = &result {
        // A player has no console to read the error in.
        engine::standalone::show_error(&format!("{err:#}"));
    }
    result
}

/// Runs the engine as the arguments say, or `embedded` (the unpacked
/// project of an exported game) as a player gets it.
fn run(embedded: Option<PathBuf>) -> anyhow::Result<()> {
    let mut args = parse_args();
    if let Some(game) = embedded {
        args.mode = Mode::Play;
        args.game = game;
        args.player = true;
        args.bench = None;
        args.export = None;
    }
    if args.mode == Mode::Launcher {
        engine::launcher::run(engine::launcher::LaunchConfig {
            quality: args.quality,
            perf_log: args.perf_log,
        })?;
        return Ok(());
    }
    log::info!(
        "{} v{} | mode: {:?} | game: {}",
        engine::NAME,
        engine::VERSION,
        args.mode,
        args.game.display()
    );
    let game = if engine::package::is_package(&args.game) {
        let (dir, manifest) = engine::package::unpack(&args.game)?;
        log::info!(
            "unpacked {} ({}) into {}",
            args.game.display(),
            manifest.name,
            dir.display()
        );
        dir
    } else {
        args.game
    };
    let mut project = Project::open(&game)?;
    if let Some(out) = &args.export {
        let count = engine::standalone::export(&project.dir, &project.settings.name, out)?;
        log::info!("exported the game {} ({count} files)", out.display());
        return Ok(());
    }
    if let Some(scene) = args.scene {
        project.settings.start_scene = scene;
    }
    let scene = project.load_start_scene()?;
    log::info!(
        "{}: {} objects in {} zones",
        project.start_scene_path().display(),
        scene.objects.len(),
        scene.zones.len()
    );

    let editor = args.mode == Mode::Editor;
    let bench = match &args.bench {
        Some(path) if !editor => Some(engine::bench::Route::load(path)?),
        _ => None,
    };
    let player = args.player && !editor;
    if editor {
        // For the launcher's list.
        engine::recent::record(&project.dir, &project.settings.name);
    }
    engine::run(engine::RunConfig {
        // As a player sees it: just the game's name.
        title: if player {
            project.settings.name.clone()
        } else {
            format!(
                "{} — {}{}",
                project.settings.name,
                engine::NAME,
                if editor { " editor" } else { "" }
            )
        },
        scene_path: project.settings.start_scene.clone(),
        game_name: project.settings.name.clone(),
        game_dir: project.dir,
        scene,
        editor,
        quality: args.quality,
        render_scale: project.settings.render_scale,
        retro: project.settings.retro,
        perf_log: args.perf_log,
        // A bench always starts from the beginning.
        new_game: args.new_game || bench.is_some(),
        player,
        bench,
    })?;
    if engine::bench::failed() {
        std::process::exit(1);
    }
    Ok(())
}
