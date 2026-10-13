//! Pictures drawn ahead of the first visitor. A refresh that found new data asks for its tracker
//! to be drawn again once the owner's wait is over (`update: { warm_after: 2m }`, two minutes
//! unless said), and every later refresh pushes the wait on: KEV, NVD and EPSS updating within
//! the same minutes are drawn for once. A thread of the process that serves the pages then draws
//! each picture an overview opens with and each its buttons offer, and the spread a thing's page
//! marks its value on; until it has, the pages show the pictures of the data before.
//!
//! The ask is a file in the tracker's directory, so a refresh in a process of its own (a cell's
//! updates run apart) asks the process that serves.

use std::path::{Path, PathBuf};

/// The file a tracker's ask is, holding when it is due.
pub const FILE: &str = ".warm";

/// How often the thread looks for asks that are due.
const LOOK_EVERY: u64 = 10;

/// Asks for the tracker's pictures to be drawn after the wait, from now.
pub fn due(dir: &Path, root: &Path) {
    let at = crate::now() + crate::autoupdate::warm_after(root);
    let _ = std::fs::write(dir.join(FILE), at.to_string());
}

/// New data is in and its pictures are not drawn yet.
pub fn pending(dir: &Path) -> bool {
    dir.join(FILE).exists()
}

fn due_at(dir: &Path) -> Option<i64> {
    std::fs::read_to_string(dir.join(FILE)).ok().and_then(|s| s.trim().parse().ok())
}

/// Draws one tracker's pictures for what its store holds now: how many were drawn.
pub fn tracker(dir: &Path, sources: &Path) -> Result<usize, String> {
    let decl = crate::trackerdecl::TrackerDecl::load(dir)?;
    let store = crate::thingstore::ThingStore::open(dir)?;
    let standing = crate::tracker::standing_of(dir);
    // What the overview and the list of things count first, which every tracker has; and what
    // each page of vendors or products lists.
    store.coverage_kept(&standing);
    for r in decl.relations.iter().filter(|r| r.part.as_deref().is_some_and(|p| !p.is_empty())) {
        store.targets_kept(&r.name, &standing);
    }
    if decl.view.show.is_empty() && decl.thing.show.is_empty() {
        return Ok(1);
    }
    let mut ctx = crate::charts::Ctx::new(&store, &decl);
    ctx.standing = standing;
    // Each source by its own title, as a page names it; its name where it has none.
    let registry = crate::tracker::registry(sources);
    ctx.sources = decl
        .members
        .iter()
        .filter_map(|m| {
            let title = registry.get(&m.dataset).and_then(|p| crate::sourcedecl::SourceDecl::load(p).ok()).map(|d| d.title).filter(|t| !t.is_empty())?;
            Some((m.dataset.clone(), title))
        })
        .collect();
    Ok(crate::charts::draw_ahead(&ctx, &decl.view.show, &decl.thing.show))
}

/// Each workspace's trackers whose wait is over, drawn one after another at the world's turn for
/// heavy work, so no refresh and no other drawing runs beside it. A tracker whose drawing failed
/// keeps no ask: its pages draw as they are asked, as they always did.
pub fn pass(workspaces: &[PathBuf]) {
    let now = crate::now();
    for root in workspaces {
        let sources = root.join("sources");
        for (_, dir) in crate::tracker::scope_registry(&root.join("trackers")) {
            let Some(at) = due_at(&dir) else { continue };
            if at > now {
                continue;
            }
            let Ok(Some(_turn)) = crate::source::heavy_turn(root, false) else { continue };
            let t = std::time::Instant::now();
            match tracker(&dir, &sources) {
                Ok(n) if n > 0 => {
                    eprintln!("{}: {n} pictures drawn ahead in {} ms", dir.display(), t.elapsed().as_millis());
                    // What drawing them took is not held on to.
                    crate::memory::give_back();
                }
                Ok(_) => {}
                Err(e) => eprintln!("{}: pictures not drawn ahead: {e}", dir.display()),
            }
            // Asked again while drawing: it stays asked, and is drawn again after its wait.
            if due_at(&dir) == Some(at) {
                let _ = std::fs::remove_file(dir.join(FILE));
            }
        }
    }
}

/// The thread a serving process draws ahead on. At its start every tracker is asked once, after
/// the wait: a new program draws its pictures anew, and the first visitor after a deploy finds
/// them drawn.
pub fn spawn(workspaces: impl Fn() -> Vec<PathBuf> + Send + 'static) {
    std::thread::spawn(move || {
        for root in workspaces() {
            for (_, dir) in crate::tracker::scope_registry(&root.join("trackers")) {
                if !pending(&dir) {
                    due(&dir, &root);
                }
            }
        }
        loop {
            std::thread::sleep(std::time::Duration::from_secs(LOOK_EVERY));
            pass(&workspaces());
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_ask_is_pushed_on_by_the_next_and_gone_once_drawn() {
        let root = std::env::temp_dir().join(format!("zetlyn-warm-{}", std::process::id()));
        let dir = root.join("trackers").join("t");
        std::fs::create_dir_all(&dir).unwrap();
        super::due(&dir, &root);
        let first = super::due_at(&dir).unwrap();
        assert!(super::pending(&dir));
        assert!(first >= crate::now() + 100, "two minutes unless the owner says otherwise");
        super::due(&dir, &root);
        assert!(super::due_at(&dir).unwrap() >= first);
        std::fs::remove_dir_all(&root).ok();
    }
}
