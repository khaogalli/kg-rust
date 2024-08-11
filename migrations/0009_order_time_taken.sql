alter table "order" add column time_taken        integer;
alter table "order" add column order_placed_time timestamptz;
alter table "order" add column order_completed_time  timestamptz;