-- Add everyday-visible contact and org-unit fields to canonical people.
--
-- `phone` carries the employee's contact number (e.g. Darwinbox
-- `personal_mobile_no`). `top_department` is the parent org unit (e.g.
-- Darwinbox `top_department`). Neither is searchable; `is_active` already
-- exists with a `true` default and is derived from source-reported
-- `employee_status` by the person write path.

BEGIN;

ALTER TABLE people ADD COLUMN phone VARCHAR(64);
ALTER TABLE people ADD COLUMN top_department VARCHAR(255);

COMMIT;
