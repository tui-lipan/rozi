-- Tokens of reports already counted, so a report the client resends after a lost answer is not
-- counted twice. A token is random, made fresh for each week's report, and kept apart from the
-- counted values: nothing here says which row of `weekly_installs` it added to. Rows are deleted
-- once their week is two weeks old.
CREATE TABLE report_tokens (
  token TEXT PRIMARY KEY NOT NULL,
  week TEXT NOT NULL
) WITHOUT ROWID;

CREATE INDEX report_tokens_week ON report_tokens (week);
