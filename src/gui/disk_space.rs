//! How full the disk recordings go to is, for the main toolbar: the disk the base directory's on (a writer could write
//! elsewhere, but it's what's chosen for them all). It's looked at once a minute, and when the directory changes,
//! each time on a thread of its own, as a disk that's asleep, or on the network, can take a while to answer.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText, Sense};

use crate::theme::Colours;

/// How often it's looked at.
const EVERY: Duration = Duration::from_secs(60);
/// How full counts as nearly full, then as all but full.
const NEARLY_FULL: f32 = 0.9;
const FULL: f32 = 0.97;
const BAR: egui::Vec2 = egui::vec2(36.0, 8.0);

/// A disk's size and what's free on it (for this user), in bytes.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Space {
    pub free: u64,
    pub total: u64,
}

impl Space {
    /// How much of it's used, from 0 to 1.
    fn used(self) -> f32 {
        if self.total == 0 { 0.0 } else { 1.0 - self.free.min(self.total) as f32 / self.total as f32 }
    }
}

/// The space on the disk `directory` is on, or will be on: it may not be there yet.
fn measure(directory: &Path) -> io::Result<Space> {
    let there = directory.ancestors().find(|path| path.exists()).unwrap_or(directory);
    let stats = fs4::statvfs(there)?;
    Ok(Space { free: stats.available_space(), total: stats.total_space() })
}

/// `bytes`, as disks are sold: "512 GB", "1.2 TB".
fn size_text(bytes: u64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1000.0 && unit < units.len() - 1 {
        size /= 1000.0;
        unit += 1;
    }
    if size < 10.0 && unit > 0 { format!("{size:.1} {}", units[unit]) } else { format!("{size:.0} {}", units[unit]) }
}

/// What's known of the disk, kept by the GUI.
#[derive(Default)]
pub struct DiskSpace {
    /// The directory last asked about, and when.
    asked: Option<(PathBuf, Instant)>,
    /// The answer to that, while it's on its way. (If a disk never answers, it isn't asked again.)
    answer: Option<Receiver<io::Result<Space>>>,
    /// The last answer.
    space: Option<Result<Space, String>>,
}

impl DiskSpace {
    /// Asks about `directory`, on a thread of its own, repainting when there's an answer.
    fn ask(&mut self, ctx: &egui::Context, directory: &Path) {
        self.asked = Some((directory.to_owned(), Instant::now()));
        let (sender, receiver) = mpsc::channel();
        let (path, ctx) = (directory.to_owned(), ctx.clone());
        let measuring = move || {
            let _ = sender.send(measure(&path));
            ctx.request_repaint();
        };
        match std::thread::Builder::new().name("measuring the disk".into()).spawn(measuring) {
            Ok(_) => self.answer = Some(receiver),
            Err(e) => self.space = Some(Err(e.to_string())),
        }
    }

    /// Takes the answer, if it's come.
    fn receive(&mut self) {
        let Some(answer) = &self.answer else { return };
        match answer.try_recv() {
            Ok(space) => {
                self.space = Some(space.map_err(|e| e.to_string()));
                self.answer = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.answer = None,
        }
    }

    /// Asks again if it's time to, or `directory`'s another, and has a frame drawn when it's next time to.
    fn keep_up(&mut self, ctx: &egui::Context, directory: &Path) {
        self.receive();
        let same = self.asked.as_ref().filter(|(asked, _)| asked == directory);
        let wait = same.map_or(Duration::ZERO, |(_, at)| EVERY.saturating_sub(at.elapsed()));
        if !wait.is_zero() {
            ctx.request_repaint_after(wait);
        } else if self.answer.is_none() {
            if cfg!(test) {
                // The same on every machine, so the toolbar's snapshots are.
                self.asked = Some((directory.to_owned(), Instant::now()));
                self.space = Some(Ok(Space { free: 512_000_000_000, total: 1_000_000_000_000 }));
            } else {
                self.ask(ctx, directory);
            }
            ctx.request_repaint_after(EVERY);
        }
    }

    /// How full the disk `directory` is on is: a bar, and what's free.
    pub fn ui(&mut self, ui: &mut egui::Ui, directory: &Path, theme: &Colours) {
        self.keep_up(ui.ctx(), directory);
        match &self.space {
            Some(Ok(space)) => {
                let used = space.used();
                let colour = if used >= FULL {
                    theme.error
                } else if used >= NEARLY_FULL {
                    theme.warning
                } else {
                    theme.text_secondary
                };
                let free = size_text(space.free);
                let tip = format!(
                    "The disk recordings go to: {free} free of {} ({:.0}% used)\n{}",
                    size_text(space.total),
                    used * 100.0,
                    directory.display(),
                );
                let (rect, bar) = ui.allocate_exact_size(BAR, Sense::hover());
                let painter = ui.painter();
                painter.rect_filled(rect, 2.0, theme.surface_sunken);
                let filled = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * used, rect.height()));
                painter.rect_filled(filled, 2.0, colour);
                bar.on_hover_text(&tip);
                let text = RichText::new(format!("{free} free"));
                let text = if used >= NEARLY_FULL { text.color(colour) } else { text.color(theme.text_secondary) };
                ui.label(text).on_hover_text(tip);
            }
            Some(Err(e)) => {
                let text = RichText::new("Disk ?").color(theme.text_disabled);
                ui.label(text).on_hover_text(format!("Couldn't tell how full the disk recordings go to is: {e}"));
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_as_disks_are_sold() {
        let sizes = [(999, "999 B"), (1_500, "1.5 KB"), (512_000_000_000, "512 GB"), (1_200_000_000_000, "1.2 TB")];
        assert!(sizes.into_iter().all(|(bytes, text)| size_text(bytes) == text));
    }

    #[test]
    fn nothing_on_a_disk_is_nothing_used() {
        assert_eq!(Space { free: 0, total: 0 }.used(), 0.0);
        assert_eq!(Space { free: 25, total: 100 }.used(), 0.75);
    }

    #[test]
    fn a_directory_not_there_yet_is_measured_on_its_disk() {
        let missing = std::env::temp_dir().join("ephys-rs-not-there").join("not-this-either");
        let space = measure(&missing).unwrap();
        assert!(space.total > 0 && space.free <= space.total, "{space:?}");
    }

    #[test]
    fn the_answer_comes_from_another_thread() {
        let ctx = egui::Context::default();
        let mut disk = DiskSpace::default();
        disk.ask(&ctx, &std::env::temp_dir());
        let deadline = Instant::now() + Duration::from_secs(5);
        while disk.answer.is_some() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
            disk.receive();
        }
        assert!(matches!(disk.space, Some(Ok(_))), "{:?}", disk.space);
    }
}
