/// Merges the regions of the database attached as `side`, and their tiles
/// and resources, into the main database. `side_version` is the side
/// database's schema version, 6 or later: a version 6 database has no
/// stale-while-revalidate or stale-if-error times, nor regions' `max_stale`.
pub(crate) fn merge_sideloaded_database_sql(side_version: i64) -> String {
    let (swr, sie, max_stale) = if side_version >= 7 {
        ("stale_while_revalidate", "stale_if_error", "sr.max_stale")
    } else {
        ("NULL AS stale_while_revalidate", "NULL AS stale_if_error", "0")
    };
    format!(
        "INSERT INTO regions (definition, description, max_stale)
    SELECT sr.definition, sr.description, MAX({max_stale})
    FROM side.regions sr
    LEFT JOIN regions r ON sr.definition = r.definition AND sr.description IS r.description
        WHERE r.definition IS NULL
    GROUP BY sr.definition, sr.description
    ORDER BY MIN(sr.id);
CREATE TEMPORARY TABLE region_mapping AS
    SELECT sr.id AS side_region_id,
        r.id AS main_region_id
    FROM side.regions sr
    JOIN regions r ON sr.definition = r.definition AND sr.description IS r.description;
REPLACE INTO tiles (id, url_template, pixel_ratio, z, x, y, expires, modified, etag, data, compressed,
                    accessed, must_revalidate, stale_while_revalidate, stale_if_error)
    SELECT t.id,
        st.url_template, st.pixel_ratio, st.z, st.x, st.y,
        st.expires, st.modified, st.etag, st.data, st.compressed, st.accessed, st.must_revalidate,
        st.stale_while_revalidate, st.stale_if_error
    FROM (SELECT DISTINCT sti.id, sti.url_template, sti.pixel_ratio, sti.z, sti.x, sti.y, sti.expires,
                 sti.modified, sti.etag, sti.data, sti.compressed, sti.accessed, sti.must_revalidate,
                 {swr}, {sie}
          FROM side.region_tiles srt JOIN side.tiles sti ON srt.tile_id = sti.id)
    AS st
    LEFT JOIN tiles t ON st.url_template = t.url_template AND st.pixel_ratio = t.pixel_ratio AND st.z = t.z AND st.x = t.x AND st.y = t.y
        WHERE t.id IS NULL
        OR st.modified > t.modified;
INSERT OR IGNORE INTO region_tiles
    SELECT rm.main_region_id, sti.id
    FROM side.region_tiles srt
    JOIN region_mapping rm ON srt.region_id = rm.side_region_id
    JOIN (SELECT t.id, st.id AS side_tile_id FROM side.tiles st
            JOIN tiles t ON st.url_template = t.url_template AND st.pixel_ratio = t.pixel_ratio AND st.z = t.z AND st.x = t.x AND st.y = t.y
    ) AS sti ON srt.tile_id = sti.side_tile_id;
REPLACE INTO resources (id, url, kind, expires, modified, etag, data, compressed, accessed, must_revalidate,
                        stale_while_revalidate, stale_if_error)
    SELECT r.id,
        sr.url, sr.kind, sr.expires, sr.modified, sr.etag,
        sr.data, sr.compressed, sr.accessed, sr.must_revalidate,
        sr.stale_while_revalidate, sr.stale_if_error
    FROM side.region_resources srr
    JOIN (SELECT id, url, kind, expires, modified, etag, data, compressed, accessed, must_revalidate,
                 {swr}, {sie}
          FROM side.resources) sr ON srr.resource_id = sr.id
    LEFT JOIN resources r ON sr.url = r.url
        WHERE r.id IS NULL
        OR sr.modified > r.modified;
INSERT OR IGNORE INTO region_resources
    SELECT rm.main_region_id, sri.id
    FROM side.region_resources srr
    JOIN region_mapping rm ON srr.region_id = rm.side_region_id
    JOIN (SELECT r.id, sr.id AS side_resource_id FROM side.resources sr
            JOIN resources r ON sr.url = r.url) AS sri ON srr.resource_id = sri.side_resource_id;
DROP TABLE region_mapping;
"
    )
}
