//! The fonts for a script from a scan of the system's font files, where the
//! platform has no font matcher: the script's preferred families, then the
//! families named for it.

use crate::scripts::Script;
use crate::source::Source;

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

    /// The fonts that may draw `script`, best first: the first of its
    /// preferred families, else a family named for it (not a monospaced
    /// one), upright and closest to regular weight.
    pub(crate) fn candidates(&self, script: &Script) -> Vec<(String, Source)> {
        let rank = |info: &fontdb::FaceInfo| -> Option<usize> {
            info.families.iter().find_map(|(family, _)| {
                script.families.iter().position(|preferred| preferred.eq_ignore_ascii_case(family)).or_else(|| {
                    (!family.contains("Mono") && script.keywords.iter().any(|keyword| family.contains(keyword)))
                        .then_some(script.families.len())
                })
            })
        };
        let mut candidates: Vec<_> = self
            .db
            .faces()
            .filter_map(|info| {
                let path = match &info.source {
                    fontdb::Source::File(path) | fontdb::Source::SharedFile(path, _) => path.clone(),
                    fontdb::Source::Binary(_) => return None,
                };
                let slanted = info.style != fontdb::Style::Normal;
                let key = (rank(info)?, slanted, info.weight.0.abs_diff(fontdb::Weight::NORMAL.0));
                let family = info.families.first().map(|(name, _)| name.clone()).unwrap_or_default();
                Some((key, family, Source::File { path, index: info.index }))
            })
            .collect();
        candidates.sort_by_key(|(key, ..)| *key);
        candidates.into_iter().map(|(_, family, source)| (family, source)).collect()
    }
}
