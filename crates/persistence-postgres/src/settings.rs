use wareboxes_application::identity::UserSettingsReadModel;
use wareboxes_domain::UserId;

use crate::db::{now_iso, Db};
use crate::PersistenceResult;

pub async fn get_user_settings(
    db: &Db,
    user_id: UserId,
) -> PersistenceResult<UserSettingsReadModel> {
    let light_mode: Option<bool> =
        sqlx::query_scalar("SELECT light_mode FROM user_settings WHERE user_id = $1")
            .bind(user_id.get())
            .fetch_optional(db)
            .await?;

    Ok(UserSettingsReadModel {
        light_mode: light_mode.unwrap_or(false),
    })
}

pub async fn upsert_user_settings(
    db: &Db,
    user_id: UserId,
    light_mode: bool,
) -> PersistenceResult<UserSettingsReadModel> {
    let now = now_iso();
    sqlx::query(
        r#"
        INSERT INTO user_settings (user_id, created, modified, light_mode)
        VALUES ($1, $2, $2, $3)
        ON CONFLICT (user_id) DO UPDATE
        SET modified = EXCLUDED.modified,
            light_mode = EXCLUDED.light_mode
        "#,
    )
    .bind(user_id.get())
    .bind(now)
    .bind(light_mode)
    .execute(db)
    .await?;

    Ok(UserSettingsReadModel { light_mode })
}
