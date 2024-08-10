create table notification_token (
    user_id              uuid        references "user" (user_id) on delete cascade not null,
    expo_push_token      text                                                      not null,
    created_at           timestamptz                                               not null default now(),
    updated_at           timestamptz
);

create table notification (
    notification_id     uuid        default uuid_generate_v4() primary key,
    recipient_id        uuid        references "user" (user_id)             on delete cascade, -- null implies broadcast
    sender_id           uuid        references "restaurant" (restaurant_id) on delete cascade, -- null implies system
    ttl_minutes         int                                                                   not null,
    title               text                                                                  not null,
    body                text                                                                  not null,
    created_at          timestamptz                                                           not null default now(),
    updated_at          timestamptz
);

select trigger_updated_at('notification_token');


