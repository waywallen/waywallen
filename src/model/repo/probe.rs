use super::*;
use sea_orm::{DbBackend, FromQueryResult, Statement};

#[derive(Debug, Clone, FromQueryResult)]
pub struct ProbeState {
    pub item_id: i64,
    pub source_fields: i32,
    pub input_kind: String,
    pub signature: Option<String>,
    pub status: Option<String>,
    pub attempted_at: Option<i64>,
    pub selected_path: Option<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub size: Option<i64>,
    pub error: Option<String>,
    pub dependencies: String,
}

pub async fn next_scan_id(db: &impl ConnectionTrait) -> Result<i64> {
    let row = db
        .query_one(Statement::from_string(
            DbBackend::Sqlite,
            "UPDATE scan_sequence SET value = value + 1 WHERE id = 1 RETURNING value".to_owned(),
        ))
        .await
        .context("allocate scan id")?
        .ok_or_else(|| Error::Internal(anyhow!("scan sequence missing")))?;
    Ok(row.try_get("", "value")?)
}

pub async fn mark_item_seen(db: &impl ConnectionTrait, id: i64, scan_id: i64) -> Result<()> {
    item::Entity::update_many()
        .col_expr(item::Column::SeenScanId, Expr::value(scan_id))
        .filter(item::Column::Id.eq(id))
        .exec(db)
        .await?;
    Ok(())
}

pub async fn probe_state(db: &impl ConnectionTrait, id: i64) -> Result<Option<ProbeState>> {
    Ok(
        ProbeState::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT * FROM item_probe WHERE item_id = ?",
            [id.into()],
        ))
        .one(db)
        .await?,
    )
}

pub async fn probe_states(db: &impl ConnectionTrait) -> Result<HashMap<i64, ProbeState>> {
    Ok(ProbeState::find_by_statement(Statement::from_string(
        DbBackend::Sqlite,
        "SELECT * FROM item_probe".to_owned(),
    ))
    .all(db)
    .await?
    .into_iter()
    .map(|value| (value.item_id, value))
    .collect())
}

pub async fn set_source_media_fields(
    db: &impl ConnectionTrait,
    id: i64,
    fields: i32,
) -> Result<()> {
    db.execute(Statement::from_sql_and_values(DbBackend::Sqlite,
        "INSERT INTO item_probe (item_id, source_fields) VALUES (?, ?) ON CONFLICT(item_id) DO UPDATE SET source_fields = excluded.source_fields WHERE source_fields != excluded.source_fields",
        [id.into(), fields.into()])).await?;
    Ok(())
}

pub async fn save_probe(db: &impl ConnectionTrait, value: &ProbeState) -> Result<()> {
    db.execute(Statement::from_sql_and_values(DbBackend::Sqlite,
        "UPDATE item_probe SET input_kind = ?, signature = ?, status = ?, attempted_at = ?, selected_path = ?, width = ?, height = ?, size = ?, error = ?, dependencies = ? WHERE item_id = ?",
        vec![value.input_kind.clone().into(), value.signature.clone().into(), value.status.clone().into(), value.attempted_at.into(), value.selected_path.clone().into(), value.width.into(), value.height.into(), value.size.into(), value.error.clone().into(), value.dependencies.clone().into(), value.item_id.into()])).await?;
    Ok(())
}
