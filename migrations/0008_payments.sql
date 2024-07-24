alter table "order" add status text not null default 'payment_pending';

create table payment (
    payment_session_id text primary key,
    order_id uuid references "order" (order_id) on delete cascade not null,
    status text not null default 'pending',
    cf_order_id text not null,
    created_at timestamptz not null default now(),
    updated_at timestamptz
);

select trigger_updated_at('payment');