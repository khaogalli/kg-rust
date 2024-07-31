alter table restaurant add column open_time timestamptz not null default '2024-07-31 9:00:00+05:30';
alter table restaurant add column close_time timestamptz not null default '2024-07-31 21:00:00+05:30';
