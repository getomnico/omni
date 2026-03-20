use crate::db::error::DatabaseError;
use crate::models::Person;
use sqlx::PgPool;
use time::OffsetDateTime;
use ulid::Ulid;

pub struct PersonUpsert {
    pub email: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PersonSearchResult {
    pub id: String,
    pub email: String,
    pub display_name: Option<String>,
    pub given_name: Option<String>,
    pub surname: Option<String>,
    pub job_title: Option<String>,
    pub department: Option<String>,
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
        if people.is_empty() {
            return Ok(0);
        }

        let mut affected = 0u64;

        for person in people {
            let id = Ulid::new().to_string();
            let result = sqlx::query(
                r#"
                INSERT INTO people (id, email, display_name, updated_at)
                VALUES ($1, $2, $3, NOW())
                ON CONFLICT (email) DO UPDATE SET
                    display_name = CASE
                        WHEN people.display_name IS NULL THEN EXCLUDED.display_name
                        WHEN EXCLUDED.display_name IS NOT NULL
                             AND length(EXCLUDED.display_name) > length(people.display_name)
                        THEN EXCLUDED.display_name
                        ELSE people.display_name
                    END,
                    updated_at = NOW()
                "#,
            )
            .bind(&id)
            .bind(&person.email)
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
        let person = sqlx::query_as::<_, Person>(
            r#"
            SELECT id, email, display_name, given_name, surname, avatar_url,
                   job_title, department, division, company_name, office_location,
                   city, state, country, employee_id, employee_type, cost_center,
                   manager_id, is_active, metadata, external_id, created_at, updated_at
            FROM people
            WHERE lower(email) = lower($1)
            "#,
        )
        .bind(email)
        .fetch_optional(&self.pool)
        .await?;

        Ok(person)
    }

    pub async fn search_people(
        &self,
        query: &str,
        limit: i64,
    ) -> Result<Vec<PersonSearchResult>, DatabaseError> {
        // Use pdb.parse with lenient mode to search across all indexed fields
        let results = sqlx::query_as::<_, PersonSearchResult>(
            r#"
            SELECT p.id, p.email, p.display_name, p.given_name, p.surname,
                   p.job_title, p.department,
                   pdb.score(p.id) AS score
            FROM people p
            WHERE p.id @@@ pdb.parse(
                query_string => $1,
                lenient => true
            )
            ORDER BY score DESC
            LIMIT $2
            "#,
        )
        .bind(query)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(results)
    }

    pub async fn is_known_person(&self, term: &str) -> Result<bool, DatabaseError> {
        let exists: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS(
                SELECT 1 FROM people
                WHERE id @@@ pdb.parse(
                    query_string => $1,
                    lenient => true
                )
            )
            "#,
        )
        .bind(term)
        .fetch_one(&self.pool)
        .await?;

        Ok(exists)
    }

    pub async fn fetch_max_updated_at(&self) -> Result<Option<OffsetDateTime>, DatabaseError> {
        let ts: Option<OffsetDateTime> = sqlx::query_scalar("SELECT MAX(updated_at) FROM people")
            .fetch_one(&self.pool)
            .await?;
        Ok(ts)
    }
}
