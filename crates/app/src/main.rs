//! Single entry point. The editor is the game with a UI on top:
//!
//!   cargo run -p app                                -> play the sandbox
//!   cargo run -p app -- --editor                    -> edit the sandbox
//!   cargo run -p app -- --editor --game ../my-game  -> edit another game project
//!
//! `--game <path>` picks the game project folder (default: `sandbox`), or a
//! project package (`.gamepack`), which is unpacked next to it and opened.
//! `--scene <path>` opens another scene than the project's start scene
//! (relative to the project folder, e.g. `scenes/hall.ron`).
//! `--quality low` uses the low preset (for integrated graphics);
//! `--perf-log` logs FPS and GPU times every couple of seconds.
//! Game projects are content-only folders; they never contain engine code.

use engine::project::Project;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Play,
    Editor,
}

struct Args {
    mode: Mode,
    game: PathBuf,
    scene: Option<PathBuf>,
    quality: engine::render::quality::Quality,
    perf_log: bool,
}

fn parse_args() -> Args {
    let mut mode = Mode::Play;
    let mut game = PathBuf::from("sandbox");
    let mut scene = None;
    let mut quality = Default::default();
    let mut perf_log = false;
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--editor" => mode = Mode::Editor,
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
            other => eprintln!("ignoring unknown argument: {other}"),
        }
    }

    Args {
        mode,
        game,
        scene,
        quality,
        perf_log,
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu_hal=error,wgpu_core=warn"),
    )
    .init();
    let args = parse_args();
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
    engine::run(engine::RunConfig {
        title: format!(
            "{} — {}{}",
            project.settings.name,
            engine::NAME,
            if editor { " editor" } else { "" }
        ),
        scene_path: project.settings.start_scene.clone(),
        game_name: project.settings.name.clone(),
        game_dir: project.dir,
        scene,
        editor,
        quality: args.quality,
        render_scale: project.settings.render_scale,
        perf_log: args.perf_log,
    })?;
    Ok(())
}
