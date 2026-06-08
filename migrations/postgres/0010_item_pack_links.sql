CREATE TABLE item_pack_links (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    master_item_id BIGINT NOT NULL REFERENCES items(id),
    single_item_id BIGINT NOT NULL REFERENCES items(id),
    inner_qty BIGINT NOT NULL CHECK (inner_qty > 1),
    notes TEXT,
    CHECK (master_item_id <> single_item_id)
);

CREATE INDEX idx_item_pack_links_master
    ON item_pack_links(master_item_id)
    WHERE deleted IS NULL;
CREATE INDEX idx_item_pack_links_single
    ON item_pack_links(single_item_id)
    WHERE deleted IS NULL;
CREATE UNIQUE INDEX idx_item_pack_links_active_unique
    ON item_pack_links(master_item_id, single_item_id, inner_qty)
    WHERE deleted IS NULL;
