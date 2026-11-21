pub(crate) const MERGE_SIDELOADED_DATABASE_SQL: &str = "INSERT INTO regions\n\
\x20  SELECT DISTINCT NULL, sr.definition, sr.description\n\
\x20   FROM side.regions sr \n\
\x20   LEFT JOIN regions r ON sr.definition = r.definition AND sr.description IS r.description\n\
\x20     WHERE r.definition IS NULL;\n\
CREATE TEMPORARY TABLE region_mapping AS\n\
\x20   SELECT sr.id AS side_region_id,\n\
\x20     r.id AS main_region_id\n\
\x20   FROM side.regions sr\n\
\x20   JOIN regions r ON sr.definition = r.definition  AND sr.description IS r.description;\n\
REPLACE INTO tiles\n\
\x20   SELECT t.id,\n\
\x20       st.url_template, st.pixel_ratio, st.z, st.x, st.y,\n\
\x20       st.expires, st.modified, st.etag, st.data, st.compressed, st.accessed, st.must_revalidate\n\
\x20   FROM (SELECT DISTINCT sti.* FROM side.region_tiles srt JOIN side.tiles sti ON srt.tile_id = sti.id)\n\
\x20   AS st\n\
\x20   LEFT JOIN tiles t ON st.url_template = t.url_template AND st.pixel_ratio = t.pixel_ratio AND st.z = t.z AND st.x = t.x AND st.y = t.y\n\
\x20       WHERE t.id IS NULL\n\
\x20       OR st.modified > t.modified;\n\
INSERT OR IGNORE INTO region_tiles\n\
\x20   SELECT rm.main_region_id, sti.id\n\
\x20   FROM side.region_tiles srt\n\
\x20   JOIN region_mapping rm ON srt.region_id = rm.side_region_id\n\
\x20   JOIN (SELECT t.id, st.id AS side_tile_id FROM side.tiles st\n\
\x20           JOIN tiles t ON st.url_template = t.url_template AND st.pixel_ratio = t.pixel_ratio AND st.z = t.z AND st.x = t.x AND st.y = t.y\n\
\x20   ) AS sti ON srt.tile_id = sti.side_tile_id;\n\
REPLACE INTO resources\n\
\x20   SELECT r.id, \n\
\x20       sr.url, sr.kind, sr.expires, sr.modified, sr.etag,\n\
\x20       sr.data, sr.compressed, sr.accessed, sr.must_revalidate\n\
\x20   FROM side.region_resources srr JOIN side.resources sr ON srr.resource_id = sr.id\n\
\x20   LEFT JOIN resources r ON sr.url = r.url\n\
\x20       WHERE r.id IS NULL\n\
\x20       OR sr.modified > r.modified;\n\
INSERT OR IGNORE INTO region_resources\n\
\x20 SELECT rm.main_region_id, sri.id\n\
\x20 FROM side.region_resources srr\n\
\x20 JOIN region_mapping rm ON srr.region_id = rm.side_region_id\n\
\x20 JOIN (SELECT r.id, sr.id AS side_resource_id FROM side.resources sr\n\
\x20         JOIN resources r ON sr.url = r.url) AS sri  ON srr.resource_id = sri.side_resource_id;\n\
\x20\n\
DROP TABLE region_mapping;\n";
