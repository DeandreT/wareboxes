-- Audit waves are executable inventory work and therefore always belong to one
-- inventory owner at one facility. Carry those dimensions on every child row so
-- authorization and foreign keys do not depend on an unscoped parent lookup.

ALTER TABLE audit_waves
    ADD COLUMN facility_id BIGINT NOT NULL,
    ADD COLUMN inventory_owner_id BIGINT NOT NULL,
    ADD CONSTRAINT audit_waves_tenant_owner_facility_id_unique
        UNIQUE (tenant_id, inventory_owner_id, facility_id, id),
    ADD CONSTRAINT audit_waves_facility_fkey
        FOREIGN KEY (tenant_id, facility_id)
        REFERENCES facilities(tenant_id, id),
    ADD CONSTRAINT audit_waves_inventory_owner_fkey
        FOREIGN KEY (tenant_id, inventory_owner_id)
        REFERENCES inventory_owners(tenant_id, id),
    ADD CONSTRAINT audit_waves_owner_facility_fkey
        FOREIGN KEY (tenant_id, inventory_owner_id, facility_id)
        REFERENCES inventory_owner_facilities(tenant_id, inventory_owner_id, facility_id);

CREATE INDEX idx_audit_waves_scope
    ON audit_waves(tenant_id, facility_id, inventory_owner_id, id)
    WHERE deleted IS NULL;

ALTER TABLE audit_wave_items
    ADD COLUMN facility_id BIGINT NOT NULL,
    ADD COLUMN inventory_owner_id BIGINT NOT NULL,
    ADD CONSTRAINT audit_wave_items_wave_scope_fkey
        FOREIGN KEY (tenant_id, inventory_owner_id, facility_id, audit_wave_id)
        REFERENCES audit_waves(tenant_id, inventory_owner_id, facility_id, id),
    ADD CONSTRAINT audit_wave_items_owner_item_fkey
        FOREIGN KEY (tenant_id, inventory_owner_id, item_id)
        REFERENCES inventory_owner_items(tenant_id, inventory_owner_id, item_id);

ALTER TABLE audit_wave_inventory_owners
    ADD COLUMN facility_id BIGINT NOT NULL,
    ADD CONSTRAINT audit_wave_inventory_owners_wave_scope_fkey
        FOREIGN KEY (tenant_id, inventory_owner_id, facility_id, audit_wave_id)
        REFERENCES audit_waves(tenant_id, inventory_owner_id, facility_id, id),
    ADD CONSTRAINT audit_wave_inventory_owners_owner_facility_fkey
        FOREIGN KEY (tenant_id, inventory_owner_id, facility_id)
        REFERENCES inventory_owner_facilities(tenant_id, inventory_owner_id, facility_id);

ALTER TABLE audit_wave_locations
    ADD COLUMN facility_id BIGINT NOT NULL,
    ADD COLUMN inventory_owner_id BIGINT NOT NULL,
    ADD CONSTRAINT audit_wave_locations_wave_scope_fkey
        FOREIGN KEY (tenant_id, inventory_owner_id, facility_id, audit_wave_id)
        REFERENCES audit_waves(tenant_id, inventory_owner_id, facility_id, id),
    ADD CONSTRAINT audit_wave_locations_facility_location_fkey
        FOREIGN KEY (tenant_id, facility_id, location_id)
        REFERENCES locations(tenant_id, facility_id, id),
    ADD CONSTRAINT audit_wave_locations_owner_facility_fkey
        FOREIGN KEY (tenant_id, inventory_owner_id, facility_id)
        REFERENCES inventory_owner_facilities(tenant_id, inventory_owner_id, facility_id);

ALTER TABLE audit_location_counts
    ADD COLUMN facility_id BIGINT NOT NULL,
    ADD COLUMN uom TEXT NOT NULL,
    ADD COLUMN revision BIGINT NOT NULL DEFAULT 1,
    ADD CONSTRAINT audit_location_counts_tenant_owner_facility_id_unique
        UNIQUE (tenant_id, inventory_owner_id, facility_id, id),
    ADD CONSTRAINT audit_location_counts_wave_scope_fkey
        FOREIGN KEY (tenant_id, inventory_owner_id, facility_id, audit_id)
        REFERENCES audit_waves(tenant_id, inventory_owner_id, facility_id, id),
    ADD CONSTRAINT audit_location_counts_facility_location_fkey
        FOREIGN KEY (tenant_id, facility_id, location_id)
        REFERENCES locations(tenant_id, facility_id, id),
    ADD CONSTRAINT audit_location_counts_owner_facility_fkey
        FOREIGN KEY (tenant_id, inventory_owner_id, facility_id)
        REFERENCES inventory_owner_facilities(tenant_id, inventory_owner_id, facility_id),
    ADD CONSTRAINT audit_location_counts_uom_check CHECK (btrim(uom) <> ''),
    ADD CONSTRAINT audit_location_counts_revision_check CHECK (revision > 0);

CREATE UNIQUE INDEX audit_location_counts_dimension_unique
    ON audit_location_counts(
        tenant_id,
        audit_id,
        inventory_owner_id,
        facility_id,
        location_id,
        item_id,
        uom,
        lot,
        expiration,
        serial
    ) NULLS NOT DISTINCT;

CREATE INDEX idx_audit_location_counts_scope
    ON audit_location_counts(tenant_id, facility_id, inventory_owner_id, audit_id, id)
    WHERE deleted IS NULL;

ALTER TABLE audit_wave_assignments
    ADD COLUMN facility_id BIGINT NOT NULL,
    ADD COLUMN inventory_owner_id BIGINT NOT NULL,
    ADD CONSTRAINT audit_wave_assignments_wave_scope_fkey
        FOREIGN KEY (tenant_id, inventory_owner_id, facility_id, audit_wave_id)
        REFERENCES audit_waves(tenant_id, inventory_owner_id, facility_id, id);
