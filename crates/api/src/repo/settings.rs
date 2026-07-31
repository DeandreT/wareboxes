use wareboxes_core::dto::UserSettings;

use crate::db::{now_iso, Db};
use crate::error::AppResult;

pub async fn get_user_settings(db: &Db, user_id: i64) -> AppResult<UserSettings> {
    let light_mode: Option<bool> =
        sqlx::query_scalar("SELECT light_mode FROM user_settings WHERE user_id = $1")
            .bind(user_id)
            .fetch_optional(db)
            .await?;

    Ok(UserSettings {
        light_mode: light_mode.unwrap_or(false),
    })
}

pub async fn upsert_user_settings(
    db: &Db,
    user_id: i64,
    settings: &UserSettings,
) -> AppResult<UserSettings> {
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
    .bind(user_id)
    .bind(now)
    .bind(settings.light_mode)
    .execute(db)
    .await?;

    get_user_settings(db, user_id).await
}
