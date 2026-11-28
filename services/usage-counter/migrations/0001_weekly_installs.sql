-- One row per week and combination of reported values; `installs` is how many machines reported
-- it. This is the only table: no request, address, or timestamp finer than the week is stored.
CREATE TABLE weekly_installs (
  week TEXT NOT NULL,
  app TEXT NOT NULL,
  version TEXT NOT NULL,
  channel TEXT NOT NULL,
  os TEXT NOT NULL,
  arch TEXT NOT NULL,
  install TEXT NOT NULL,
  age INTEGER NOT NULL,
  installs INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (week, app, version, channel, os, arch, install, age)
) WITHOUT ROWID;
