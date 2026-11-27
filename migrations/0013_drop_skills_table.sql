-- 0013_drop_skills_table.sql
-- Drop legacy custom skills table and its indexes

DROP INDEX IF EXISTS idx_skills_is_active;
DROP INDEX IF EXISTS idx_skills_name;
DROP TABLE IF EXISTS skills;
