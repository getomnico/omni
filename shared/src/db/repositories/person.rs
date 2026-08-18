use crate::db::error::DatabaseError;
use crate::models::{Person, PersonSyncRecord};
use serde_json::Value as JsonValue;
use sqlx::{PgConnection, PgPool};
use time::OffsetDateTime;
use ulid::Ulid;

/// Advisory-lock namespace for source-scoped person mutations.
/// Advisory lock namespace shared by connector-manager event ingestion,
/// source cleanup, and indexer person mutation application. Per-source
/// serialization keeps emission, deletion, and person writes mutually
/// exclusive so a deleted source cannot receive new mutations.
pub const SOURCE_MUTATION_LOCK_NAMESPACE: i32 = 0x5045_5253;

pub struct PersonUpsert {
    pub email: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct PersonSyncResult {
    pub canonical_created: bool,
    pub source_entry_upserted: bool,
    pub source_entry_removed: bool,
}

/// Optional structured filters applied on top of the free-text people query.
/// All fields are directory-visible and BM25-indexed.
#[derive(Debug, Clone, Default)]
pub struct PersonSearchFilter {
    pub department: Option<String>,
    pub office_location: Option<String>,
    pub work_country: Option<String>,
    pub employee_type: Option<String>,
    pub manager: Option<String>,
    pub job_title: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PersonSearchResult {
    pub id: String,
    pub email: String,
    pub display_name: Option<String>,
    pub given_name: Option<String>,
    pub middle_name: Option<String>,
    pub surname: Option<String>,
    pub job_title: Option<String>,
    pub department: Option<String>,
    pub division: Option<String>,
    pub company_name: Option<String>,
    pub office_location: Option<String>,
    pub work_country: Option<String>,
    pub employee_id: Option<String>,
    pub employee_type: Option<String>,
    pub cost_center: Option<String>,
    pub grade: Option<String>,
    pub band: Option<String>,
    pub confirmation_status: Option<String>,
    pub employment_start_date: Option<chrono::NaiveDate>,
    pub employment_end_date: Option<chrono::NaiveDate>,
    pub manager_id: Option<String>,
    pub manager_name: Option<String>,
    pub phone: Option<String>,
    pub avatar_url: Option<String>,
    pub top_department: Option<String>,
    pub score: f32,
}

pub struct PersonRepository {
    pool: PgPool,
}

impl PersonRepository {
    pub fn new(pool: &PgPool) -> Self {
        Self { pool: pool.clone() }
    }

    pub async fn upsert_people_batch(&self, people: &[PersonUpsert]) -> Result<u64, DatabaseError> {
        let mut affected = 0;
        for person in people {
            let email = normalize_email(&person.email)?;
            let result = sqlx::query(
                r#"
                INSERT INTO people (id, email, display_name, updated_at)
                VALUES ($1, $2, $3, NOW())
                ON CONFLICT (email) DO UPDATE SET
                    display_name = CASE
                        WHEN people.source_data <> '{}'::jsonb THEN people.display_name
                        WHEN people.display_name IS NULL THEN EXCLUDED.display_name
                        WHEN EXCLUDED.display_name IS NOT NULL
                             AND length(EXCLUDED.display_name) > length(people.display_name)
                        THEN EXCLUDED.display_name
                        ELSE people.display_name
                    END,
                    updated_at = NOW()
                "#,
            )
            .bind(Ulid::new().to_string())
            .bind(email)
            .bind(&person.display_name)
            .execute(&self.pool)
            .await?;
            affected += result.rows_affected();
        }
        Ok(affected)
    }

    pub async fn fetch_person_by_email(
        &self,
        email: &str,
    ) -> Result<Option<Person>, DatabaseError> {
        Ok(sqlx::query_as::<_, Person>(
            r#"
            SELECT id, email, display_name, given_name, surname, avatar_url,
                   job_title, department, division, company_name, office_location,
                   city, state, country, employee_id, employee_type, cost_center,
                   manager_id, is_active, metadata, external_id, created_at, updated_at
            FROM people WHERE lower(email) = lower($1)
            "#,
        )
        .bind(email.trim())
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn search_people(
        &self,
        query: &str,
        limit: i64,
        filter: &PersonSearchFilter,
    ) -> Result<Vec<PersonSearchResult>, DatabaseError> {
        Ok(sqlx::query_as::<_, PersonSearchResult>(
            r#"
            SELECT p.id, p.email, p.display_name, p.given_name, p.middle_name,
                   p.surname, p.job_title, p.department, p.division, p.company_name,
                   p.office_location, p.work_country, p.employee_id, p.employee_type,
                   p.cost_center, p.grade, p.band, p.confirmation_status,
                   p.employment_start_date, p.employment_end_date,
                   p.manager_id, m.display_name AS manager_name,
                   p.phone, p.avatar_url, p.top_department,
                   pdb.score(p.id) AS score
            FROM people p
            LEFT JOIN people m ON m.id = p.manager_id
            WHERE p.is_active = true
              AND p.id @@@ pdb.parse(query_string => $1, lenient => true)
              AND ($2::text IS NULL OR p.department @@@ $2)
              AND ($3::text IS NULL OR p.office_location @@@ $3)
              AND ($4::text IS NULL OR p.work_country @@@ $4)
              AND ($5::text IS NULL OR p.employee_type @@@ $5)
              AND ($6::text IS NULL OR m.display_name @@@ $6
                    OR m.employee_id = $6)
              AND ($7::text IS NULL OR p.job_title @@@ $7)
            ORDER BY score DESC LIMIT $8
            "#,
        )
        .bind(query)
        .bind(filter.department.as_deref())
        .bind(filter.office_location.as_deref())
        .bind(filter.work_country.as_deref())
        .bind(filter.employee_type.as_deref())
        .bind(filter.manager.as_deref())
        .bind(filter.job_title.as_deref())
        .bind(limit)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn is_known_person(&self, term: &str) -> Result<bool, DatabaseError> {
        Ok(sqlx::query_scalar(
            r#"
            SELECT EXISTS(
                SELECT 1 FROM people
                WHERE is_active = true
                  AND id @@@ pdb.parse(query_string => $1, lenient => true)
            )
            "#,
        )
        .bind(term)
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn fetch_max_updated_at(&self) -> Result<Option<OffsetDateTime>, DatabaseError> {
        Ok(sqlx::query_scalar("SELECT MAX(updated_at) FROM people")
            .fetch_one(&self.pool)
            .await?)
    }

    pub async fn apply_person_sync(
        &self,
        source_id: &str,
        person: &PersonSyncRecord,
    ) -> Result<PersonSyncResult, DatabaseError> {
        validate_source_id(source_id)?;
        validate_person(person)?;
        let email = normalize_email(&person.email)?;
        let source_value = source_value(person)?;
        let mut tx = self.pool.begin().await?;
        lock_source_person_mutations(&mut tx, source_id).await?;

        let existing: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT id, source_data -> $1 ->> 'external_id' FROM people WHERE lower(email)=lower($2) FOR UPDATE",
        )
        .bind(source_id)
        .bind(&email)
        .fetch_optional(&mut *tx)
        .await?;
        let old_external_id = existing.as_ref().and_then(|(_, value)| value.clone());
        let person_id: String = sqlx::query_scalar(
            r#"
            INSERT INTO people (
                id, email, display_name, given_name, middle_name, surname,
                job_title, department, division, company_name, office_location,
                work_country, employee_id, employee_type, cost_center, grade,
                band, confirmation_status, employment_start_date,
                employment_end_date, phone, top_department, is_active,
                source_data, updated_at
            ) VALUES (
                $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,
                $19::date,$20::date,$21,$22,true,jsonb_build_object($23::text,$24::jsonb),NOW()
            )
            ON CONFLICT (email) DO UPDATE SET
                source_data=jsonb_set(people.source_data,ARRAY[$23::text],$24::jsonb,true),
                is_active=true, updated_at=NOW()
            RETURNING id
            "#,
        )
        .bind(Ulid::new().to_string())
        .bind(&email)
        .bind(&person.display_name)
        .bind(&person.given_name)
        .bind(&person.middle_name)
        .bind(&person.surname)
        .bind(&person.job_title)
        .bind(&person.department)
        .bind(&person.division)
        .bind(&person.company_name)
        .bind(&person.office_location)
        .bind(&person.work_country)
        .bind(&person.employee_id)
        .bind(&person.employee_type)
        .bind(&person.cost_center)
        .bind(&person.grade)
        .bind(&person.band)
        .bind(&person.confirmation_status)
        .bind(&person.employment_start_date)
        .bind(&person.employment_end_date)
        .bind(&person.phone)
        .bind(&person.top_department)
        .bind(source_id)
        .bind(source_value)
        .fetch_one(&mut *tx)
        .await?;

        let mut affected = vec![person_id];
        for external_id in [
            old_external_id.as_deref(),
            Some(person.external_id.as_str()),
        ]
        .into_iter()
        .flatten()
        {
            let mut subjects: Vec<String> = sqlx::query_scalar(
                "SELECT id FROM people WHERE source_data -> $1 ->> 'manager_external_id'=$2",
            )
            .bind(source_id)
            .bind(external_id)
            .fetch_all(&mut *tx)
            .await?;
            affected.append(&mut subjects);
        }
        affected.sort();
        affected.dedup();
        refresh_canonical_fields(&mut tx, &affected).await?;
        tx.commit().await?;
        Ok(PersonSyncResult {
            canonical_created: existing.is_none(),
            source_entry_upserted: true,
            source_entry_removed: false,
        })
    }

    pub async fn apply_person_deleted(
        &self,
        source_id: &str,
        email: &str,
    ) -> Result<PersonSyncResult, DatabaseError> {
        validate_source_id(source_id)?;
        let email = normalize_email(email)?;
        let mut tx = self.pool.begin().await?;
        lock_source_person_mutations(&mut tx, source_id).await?;
        let existing: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT id, source_data -> $1 ->> 'external_id' FROM people WHERE email=$2 FOR UPDATE",
        )
        .bind(source_id)
        .bind(&email)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((person_id, external_id)) = existing else {
            tx.rollback().await?;
            return Ok(PersonSyncResult::default());
        };
        let removed = external_id.is_some();
        if !removed {
            tx.rollback().await?;
            return Ok(PersonSyncResult::default());
        }
        sqlx::query("UPDATE people SET source_data=source_data-$1,updated_at=NOW() WHERE id=$2")
            .bind(source_id)
            .bind(&person_id)
            .execute(&mut *tx)
            .await?;
        let mut affected = vec![person_id];
        if let Some(external_id) = external_id {
            let mut subjects: Vec<String> = sqlx::query_scalar(
                "SELECT id FROM people WHERE source_data -> $1 ->> 'manager_external_id'=$2",
            )
            .bind(source_id)
            .bind(external_id)
            .fetch_all(&mut *tx)
            .await?;
            affected.append(&mut subjects);
        }
        affected.sort();
        affected.dedup();
        refresh_canonical_fields(&mut tx, &affected).await?;
        tx.commit().await?;
        Ok(PersonSyncResult {
            source_entry_removed: true,
            ..Default::default()
        })
    }
}

async fn lock_source_person_mutations(
    conn: &mut PgConnection,
    source_id: &str,
) -> Result<(), DatabaseError> {
    sqlx::query("SELECT pg_advisory_xact_lock($1, hashtext($2))")
        .bind(SOURCE_MUTATION_LOCK_NAMESPACE)
        .bind(source_id)
        .execute(conn)
        .await?;
    Ok(())
}

async fn refresh_canonical_fields(
    conn: &mut PgConnection,
    person_ids: &[String],
) -> Result<(), DatabaseError> {
    sqlx::query(
        r#"
        UPDATE people p SET
            display_name=(SELECT value->>'display_name' FROM jsonb_each(p.source_data) WHERE NULLIF(value->>'display_name','') IS NOT NULL ORDER BY key LIMIT 1),
            given_name=(SELECT value->>'given_name' FROM jsonb_each(p.source_data) WHERE NULLIF(value->>'given_name','') IS NOT NULL ORDER BY key LIMIT 1),
            middle_name=(SELECT value->>'middle_name' FROM jsonb_each(p.source_data) WHERE NULLIF(value->>'middle_name','') IS NOT NULL ORDER BY key LIMIT 1),
            surname=(SELECT value->>'surname' FROM jsonb_each(p.source_data) WHERE NULLIF(value->>'surname','') IS NOT NULL ORDER BY key LIMIT 1),
            job_title=(SELECT value->>'job_title' FROM jsonb_each(p.source_data) WHERE NULLIF(value->>'job_title','') IS NOT NULL ORDER BY key LIMIT 1),
            department=(SELECT value->>'department' FROM jsonb_each(p.source_data) WHERE NULLIF(value->>'department','') IS NOT NULL ORDER BY key LIMIT 1),
            division=(SELECT value->>'division' FROM jsonb_each(p.source_data) WHERE NULLIF(value->>'division','') IS NOT NULL ORDER BY key LIMIT 1),
            company_name=(SELECT value->>'company_name' FROM jsonb_each(p.source_data) WHERE NULLIF(value->>'company_name','') IS NOT NULL ORDER BY key LIMIT 1),
            office_location=(SELECT value->>'office_location' FROM jsonb_each(p.source_data) WHERE NULLIF(value->>'office_location','') IS NOT NULL ORDER BY key LIMIT 1),
            work_country=(SELECT value->>'work_country' FROM jsonb_each(p.source_data) WHERE NULLIF(value->>'work_country','') IS NOT NULL ORDER BY key LIMIT 1),
            employee_id=(SELECT value->>'employee_id' FROM jsonb_each(p.source_data) WHERE NULLIF(value->>'employee_id','') IS NOT NULL ORDER BY key LIMIT 1),
            employee_type=(SELECT value->>'employee_type' FROM jsonb_each(p.source_data) WHERE NULLIF(value->>'employee_type','') IS NOT NULL ORDER BY key LIMIT 1),
            cost_center=(SELECT value->>'cost_center' FROM jsonb_each(p.source_data) WHERE NULLIF(value->>'cost_center','') IS NOT NULL ORDER BY key LIMIT 1),
            grade=(SELECT value->>'grade' FROM jsonb_each(p.source_data) WHERE NULLIF(value->>'grade','') IS NOT NULL ORDER BY key LIMIT 1),
            band=(SELECT value->>'band' FROM jsonb_each(p.source_data) WHERE NULLIF(value->>'band','') IS NOT NULL ORDER BY key LIMIT 1),
            confirmation_status=(SELECT value->>'confirmation_status' FROM jsonb_each(p.source_data) WHERE NULLIF(value->>'confirmation_status','') IS NOT NULL ORDER BY key LIMIT 1),
            employment_start_date=(SELECT (value->>'employment_start_date')::date FROM jsonb_each(p.source_data) WHERE NULLIF(value->>'employment_start_date','') IS NOT NULL ORDER BY key LIMIT 1),
            employment_end_date=(SELECT (value->>'employment_end_date')::date FROM jsonb_each(p.source_data) WHERE NULLIF(value->>'employment_end_date','') IS NOT NULL ORDER BY key LIMIT 1),
            phone=(SELECT value->>'phone' FROM jsonb_each(p.source_data) WHERE NULLIF(value->>'phone','') IS NOT NULL ORDER BY key LIMIT 1),
            top_department=(SELECT value->>'top_department' FROM jsonb_each(p.source_data) WHERE NULLIF(value->>'top_department','') IS NOT NULL ORDER BY key LIMIT 1),
            manager_id=(
                SELECT manager.id FROM jsonb_each(p.source_data) subject
                JOIN people manager ON manager.source_data ? subject.key
                 AND manager.source_data -> subject.key ->> 'external_id'=subject.value->>'manager_external_id'
                WHERE NULLIF(subject.value->>'manager_external_id','') IS NOT NULL
                ORDER BY subject.key LIMIT 1
            ),
            is_active=COALESCE(
                (SELECT (value->>'is_active')::boolean FROM jsonb_each(p.source_data) WHERE value ? 'is_active' ORDER BY key LIMIT 1),
                p.source_data<>'{}'::jsonb
            ),
            updated_at=NOW()
        WHERE p.id=ANY($1::text[])
        "#,
    )
    .bind(person_ids)
    .execute(conn)
    .await?;
    Ok(())
}

fn validate_source_id(source_id: &str) -> Result<(), DatabaseError> {
    if source_id.is_empty() || source_id.trim() != source_id {
        return Err(DatabaseError::InvalidInput("invalid source_id".into()));
    }
    Ok(())
}

fn validate_person(person: &PersonSyncRecord) -> Result<(), DatabaseError> {
    if person.external_id.is_empty() || person.external_id.trim() != person.external_id {
        return Err(DatabaseError::InvalidInput("invalid external_id".into()));
    }
    if person.manager_external_id.as_deref() == Some(person.external_id.as_str()) {
        return Err(DatabaseError::InvalidInput(
            "person cannot manage themselves".into(),
        ));
    }
    if let Some(manager) = &person.manager_external_id
        && (manager.is_empty() || manager.trim() != manager)
    {
        return Err(DatabaseError::InvalidInput(
            "invalid manager_external_id".into(),
        ));
    }
    for (label, value) in [
        ("employment_start_date", &person.employment_start_date),
        ("employment_end_date", &person.employment_end_date),
    ] {
        if let Some(value) = value {
            time::Date::parse(
                value,
                &time::format_description::well_known::Iso8601::DEFAULT,
            )
            .map_err(|_| DatabaseError::InvalidInput(format!("invalid {label}")))?;
        }
    }
    if let Some(value) = &person.source_updated_at {
        time::OffsetDateTime::parse(
            value,
            &time::format_description::well_known::Iso8601::DEFAULT,
        )
        .map_err(|_| DatabaseError::InvalidInput("invalid source_updated_at".into()))?;
    }
    normalize_email(&person.email)?;
    Ok(())
}

fn normalize_email(email: &str) -> Result<String, DatabaseError> {
    let normalized = email.trim().to_lowercase();
    let valid = normalized.split_once('@').is_some_and(|(local, domain)| {
        !local.is_empty() && domain.contains('.') && !domain.ends_with('.')
    });
    if !valid {
        return Err(DatabaseError::InvalidInput("invalid business email".into()));
    }
    Ok(normalized)
}

fn source_value(person: &PersonSyncRecord) -> Result<JsonValue, DatabaseError> {
    let mut value = serde_json::to_value(person)
        .map_err(|error| DatabaseError::InvalidInput(error.to_string()))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| DatabaseError::InvalidInput("person must serialize as an object".into()))?;
    object.remove("email");
    object.retain(|_, field| !field.is_null());
    Ok(value)
}
