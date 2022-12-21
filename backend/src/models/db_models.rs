

use uuid::Uuid;
use chrono::DateTime;
use chrono::offset::Utc;
use diesel::{Identifiable, Queryable};
use serde::Serialize;
use serde_json::Value;
use crate::db::schema::{contexts, dimensions, global_config, overrides};

#[derive(Queryable, Debug, Identifiable, Serialize)]
#[diesel(table_name = contexts)]
#[diesel(primary_key(key))]
pub struct Contexts {
    pub key: String,
    pub value: Value,
    pub last_modified: DateTime<Utc>,
    pub created_on: DateTime<Utc>,
}

#[derive(Queryable, Debug, Identifiable, Serialize)]
#[diesel(table_name = dimensions)]
#[diesel(primary_key(dimension))]
pub struct Dimension {
    pub uuid: Uuid,
    pub dimension: String,
    pub priority: i32,
    pub last_modified: DateTime<Utc>,
    pub created_on: DateTime<Utc>,
}

#[derive(Queryable, Debug, Identifiable, Serialize)]
#[diesel(table_name = global_config)]
#[diesel(primary_key(key))]
pub struct GlobalConfig {
    pub uuid: Uuid,
    pub key: String,
    pub value: Value,
    pub last_modified: DateTime<Utc>,
    pub created_on: DateTime<Utc>,
}

#[derive(Queryable, Debug, Identifiable, Serialize)]
#[diesel(table_name = overrides)]
#[diesel(primary_key(key))]
pub struct Overrides {
    pub key: String,
    pub value: Value,
    pub last_modified: DateTime<Utc>,
    pub created_on: DateTime<Utc>,
}