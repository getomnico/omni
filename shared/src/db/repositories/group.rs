use crate::{db::error::DatabaseError, models::Group};
use sqlx::PgPool;
use ulid::Ulid;

pub struct GroupRepository {
    pool: PgPool,
}

impl GroupRepository {
    pub fn new(pool: &PgPool) -> Self {
        Self { pool: pool.clone() }
    }

    /// Upsert a group by (source_id, email), returning the group record
    pub async fn upsert_group(
        &self,
        source_id: &str,
        email: &str,
        display_name: Option<&str>,
        description: Option<&str>,
    ) -> Result<Group, DatabaseError> {
        let id = Ulid::new().to_string();
        let group = sqlx::query_as::<_, Group>(
            r#"
            INSERT INTO groups (id, source_id, email, display_name, description, synced_at)
            VALUES ($1, $2, $3, $4, $5, NOW())
            ON CONFLICT (source_id, email)
            DO UPDATE SET
                display_name = COALESCE(EXCLUDED.display_name, groups.display_name),
                description = COALESCE(EXCLUDED.description, groups.description),
                synced_at = NOW()
            RETURNING id, source_id, email, display_name, description, synced_at
            "#,
        )
        .bind(&id)
        .bind(source_id)
        .bind(email)
        .bind(display_name)
        .bind(description)
        .fetch_one(&self.pool)
        .await?;

        Ok(group)
    }

    /// Replace all members of a group. Deletes existing memberships and inserts new ones.
    pub async fn sync_group_members(
        &self,
        group_id: &str,
        member_emails: &[String],
    ) -> Result<usize, DatabaseError> {
        let mut tx = self.pool.begin().await?;

        sqlx::query("DELETE FROM group_memberships WHERE group_id = $1")
            .bind(group_id)
            .execute(&mut *tx)
            .await?;

        if member_emails.is_empty() {
            tx.commit().await?;
            return Ok(0);
        }

        let ids: Vec<String> = member_emails
            .iter()
            .map(|_| Ulid::new().to_string())
            .collect();
        let group_ids: Vec<String> = member_emails.iter().map(|_| group_id.to_string()).collect();

        let count = sqlx::query(
            r#"
            INSERT INTO group_memberships (id, group_id, member_email, synced_at)
            SELECT * FROM UNNEST($1::text[], $2::text[], $3::text[], $4::timestamptz[])
                AS t(id, group_id, member_email, synced_at)
            ON CONFLICT (group_id, member_email) DO UPDATE SET synced_at = NOW()
            "#,
        )
        .bind(&ids)
        .bind(&group_ids)
        .bind(member_emails)
        .bind(&vec![
            sqlx::types::time::OffsetDateTime::now_utc();
            member_emails.len()
        ])
        .execute(&mut *tx)
        .await?
        .rows_affected();

        tx.commit().await?;

        Ok(count as usize)
    }

    /// Find all group emails that a user belongs to (across all sources)
    pub async fn find_groups_for_user(
        &self,
        user_email: &str,
    ) -> Result<Vec<String>, DatabaseError> {
        let group_emails: Vec<String> = sqlx::query_scalar(
            r#"
            SELECT DISTINCT g.email
            FROM groups g
            JOIN group_memberships gm ON g.id = gm.group_id
            WHERE lower(gm.member_email) = lower($1)
            "#,
        )
        .bind(user_email)
        .fetch_all(&self.pool)
        .await?;

        Ok(group_emails)
    }

    /// Delete all groups (and cascade memberships) for a source
    pub async fn delete_by_source(&self, source_id: &str) -> Result<u64, DatabaseError> {
        let result = sqlx::query("DELETE FROM groups WHERE source_id = $1")
            .bind(source_id)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected())
    }
}
