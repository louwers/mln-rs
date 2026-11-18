//! The fonts for a character from a scan of the system's font files, where
//! the platform has no font matcher: the family asked for, then the
//! preferred families of the character's script and the families named for
//! it, then common sans-serif families.

use crate::Query;
use crate::scripts::SCRIPTS;
use crate::source::Source;

/// The sans-serif families to try for a character of no known script.
const SANS_SERIF: [&str; 8] =
    ["Noto Sans", "DejaVu Sans", "Roboto", "Liberation Sans", "Arial", "Helvetica", "Segoe UI", "Open Sans"];

/// The system's fonts, scanned once.
pub(crate) struct Scan {
    db: fontdb::Database,
}

impl Scan {
    pub(crate) fn new() -> Scan {
        let mut db = fontdb::Database::new();
        // fontdb knows no font directory on Android.
        if cfg!(target_os = "android") {
            db.load_fonts_dir("/system/fonts");
        } else {
            db.load_system_fonts();
        }
        Scan { db }
    }

    /// The fonts that may draw `query`'s character, best first: the family
    /// asked for, else a preferred family of the character's script, else a
    /// family named for it (not a monospaced one), else a common sans-serif
    /// family; of each, the style and the weight closest to those asked for.
    pub(crate) fn candidates(&self, query: &Query<'_>) -> Vec<(String, Source)> {
        let probe = u32::from(query.probe);
        let scripts: Vec<_> = SCRIPTS
            .iter()
            .filter(|script| script.ranges.iter().any(|&(first, last)| (first..=last).contains(&probe)))
            .collect();
        let rank = |family: &str| -> Option<usize> {
            if query.family.is_some_and(|asked| asked.eq_ignore_ascii_case(family)) {
                return Some(0);
            }
            let mut rank = 1;
            for script in &scripts {
                if let Some(position) =
                    script.families.iter().position(|preferred| preferred.eq_ignore_ascii_case(family))
                {
                    return Some(rank + position);
                }
                rank += script.families.len();
                if !family.contains("Mono") && script.keywords.iter().any(|keyword| family.contains(keyword)) {
                    return Some(rank);
                }
                rank += 1;
            }
            SANS_SERIF.iter().position(|sans| sans.eq_ignore_ascii_case(family)).map(|position| rank + position)
        };
        let mut candidates: Vec<_> = self
            .db
            .faces()
            .filter_map(|info| {
                let path = match &info.source {
                    fontdb::Source::File(path) | fontdb::Source::SharedFile(path, _) => path.clone(),
                    fontdb::Source::Binary(_) => return None,
                };
                let rank = info.families.iter().find_map(|(family, _)| rank(family))?;
                let slanted = info.style != fontdb::Style::Normal;
                let key = (rank, slanted != query.italic, info.weight.0.abs_diff(query.weight));
                let family = info.families.first().map(|(name, _)| name.clone()).unwrap_or_default();
                Some((key, family, Source::File { path, index: info.index }))
            })
            .collect();
        candidates.sort_by_key(|(key, ..)| *key);
        candidates.into_iter().map(|(_, family, source)| (family, source)).collect()
    }
}
