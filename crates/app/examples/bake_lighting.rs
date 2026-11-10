//! Bakes a scene's lighting without the editor, the way BAKE ▸ LIGHTING
//! does, and reports how long it took (for measuring bakes):
//! `cargo run --release -p app --example bake_lighting -- <game folder> <scene>`
//! e.g. `... -- sandbox scenes/test_level.ron`. Writes the scene's
//! `.lightmap` next to it.

use engine::{assets::Assets, lightmap, scene::Scene};
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let mut args = std::env::args().skip(1);
    let (Some(game), Some(scene)) = (args.next(), args.next()) else {
        return Err("usage: bake_lighting <game folder> <scene>".into());
    };
    let game = PathBuf::from(game);
    let file = game.join(&scene);
    let scene = Scene::load(&file)?;
    let mut assets = Assets::new(game);
    let mut job = lightmap::start(&scene, &mut assets, lightmap::path_for(&file))?;
    loop {
        if let Some(result) = job.finished() {
            println!("baked {}", result?.display());
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}
