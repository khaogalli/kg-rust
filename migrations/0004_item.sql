create table item (
    item_id       uuid primary key                                default uuid_generate_v1mc(),
    restaurant_id uuid references restaurant (restaurant_id) on delete cascade,
    name          text                                   not null,
    description   text,
    price         int                                    not null,
    created_at    timestamptz                            not null default now(),
    updated_at    timestamptz
);

SELECT trigger_updated_at('item');