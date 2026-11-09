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
//! Game projects are content-only folders; they never contain engine code.
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
}

fn parse_args() -> Args {
    let mut mode = Mode::Play;
    let mut game = PathBuf::from("sandbox");
    let mut scene = None;
    let mut quality = Default::default();
    let mut perf_log = false;
    let mut new_game = false;
    let mut player = false;
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
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu_hal=error,wgpu_core=warn"),
    )
    .init();
    let args = parse_args();
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
        perf_log: args.perf_log,
        new_game: args.new_game,
        player,
    })?;
    Ok(())
}
