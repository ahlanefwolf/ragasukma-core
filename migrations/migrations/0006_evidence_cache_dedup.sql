-- 0006_evidence_cache_dedup.sql
-- Evidence cache accumulated a new row on every successful file read: `save_evidence`
-- always INSERTed and there was no uniqueness guarantee, so the table grew without
-- bound and most rows ended up orphaned (never linked to a goal).
--
-- This migration collapses duplicates down to the most recently updated row per
-- (project_id, file_path, line range) and then enforces uniqueness so future writes
-- must upsert instead of blindly append.

PRAGMA foreign_keys = ON;

-- 1. Repoint goal_evidence links at the surviving row before deleting duplicates,
--    so no goal silently loses its context.
UPDATE OR IGNORE goal_evidence
SET evidence_id = (
    SELECT e_keep.id
    FROM evidence_cache e_dup
    JOIN evidence_cache e_keep
      ON e_keep.project_id = e_dup.project_id
     AND e_keep.file_path = e_dup.file_path
     AND COALESCE(e_keep.start_line, -1) = COALESCE(e_dup.start_line, -1)
     AND COALESCE(e_keep.end_line, -1) = COALESCE(e_dup.end_line, -1)
    WHERE e_dup.id = goal_evidence.evidence_id
    ORDER BY e_keep.updated_at DESC, e_keep.id ASC
    LIMIT 1
)
WHERE evidence_id IN (
    SELECT e.id FROM evidence_cache e
    WHERE e.id <> (
        SELECT k.id FROM evidence_cache k
        WHERE k.project_id = e.project_id
          AND k.file_path = e.file_path
          AND COALESCE(k.start_line, -1) = COALESCE(e.start_line, -1)
          AND COALESCE(k.end_line, -1) = COALESCE(e.end_line, -1)
        ORDER BY k.updated_at DESC, k.id ASC
        LIMIT 1
    )
);

-- 2. Drop the duplicate evidence rows, keeping the newest per file/line range.
DELETE FROM evidence_cache
WHERE id NOT IN (
    SELECT id FROM (
        SELECT id,
               ROW_NUMBER() OVER (
                   PARTITION BY project_id, file_path,
                                COALESCE(start_line, -1), COALESCE(end_line, -1)
                   ORDER BY updated_at DESC, id ASC
               ) AS rn
        FROM evidence_cache
    )
    WHERE rn = 1
);

-- 3. Enforce one row per (project, file, line range) so writes must upsert.
CREATE UNIQUE INDEX IF NOT EXISTS idx_evidence_unique_slice
    ON evidence_cache(project_id, file_path, COALESCE(start_line, -1), COALESCE(end_line, -1));
