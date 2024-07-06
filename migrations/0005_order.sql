create table "order" (
    order_id      uuid primary key                                default uuid_generate_v1mc(),
    restaurant_id uuid references restaurant (restaurant_id) on delete cascade not null,
    user_id       uuid references "user" (user_id) on delete cascade not null,
    total         int                                    not null,
    pending       bool                                   not null default true,
    created_at    timestamptz                            not null default now(),
    updated_at    timestamptz
);

SELECT trigger_updated_at('order');

create table order_item (
    order_id uuid references "order" (order_id) on delete cascade,
    item_name text not null,
    item_price int not null,
    quantity int not null
);