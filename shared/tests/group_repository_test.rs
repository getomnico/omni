#[cfg(test)]
mod tests {
    use shared::db::repositories::GroupRepository;
    use shared::test_environment::TestEnvironment;

    const TEST_SOURCE_ID: &str = "01JGF7V3E0Y2R1X8P5Q7W9T4N7";

    #[tokio::test]
    async fn test_sync_group_members_replaces_old() {
        let env = TestEnvironment::new().await.unwrap();
        let repo = GroupRepository::new(env.db_pool.pool());

        let group = repo
            .upsert_group(TEST_SOURCE_ID, "eng@co.com", Some("Engineering"), None)
            .await
            .unwrap();

        // First sync: members A and B
        let count = repo
            .sync_group_members(&group.id, &["alice@co.com".into(), "bob@co.com".into()])
            .await
            .unwrap();
        assert_eq!(count, 2);

        // Second sync: members B and C (A should be removed)
        let count = repo
            .sync_group_members(&group.id, &["bob@co.com".into(), "charlie@co.com".into()])
            .await
            .unwrap();
        assert_eq!(count, 2);

        // Verify: alice should no longer be a member, bob and charlie should be
        let alice_groups = repo.find_groups_for_user("alice@co.com").await.unwrap();
        assert!(alice_groups.is_empty(), "alice should have been removed");

        let bob_groups = repo.find_groups_for_user("bob@co.com").await.unwrap();
        assert_eq!(bob_groups, vec!["eng@co.com"]);

        let charlie_groups = repo.find_groups_for_user("charlie@co.com").await.unwrap();
        assert_eq!(charlie_groups, vec!["eng@co.com"]);
    }

    #[tokio::test]
    async fn test_find_groups_for_user_case_insensitive() {
        let env = TestEnvironment::new().await.unwrap();
        let repo = GroupRepository::new(env.db_pool.pool());

        let group1 = repo
            .upsert_group(TEST_SOURCE_ID, "eng@co.com", Some("Engineering"), None)
            .await
            .unwrap();
        let group2 = repo
            .upsert_group(TEST_SOURCE_ID, "all@co.com", Some("All"), None)
            .await
            .unwrap();

        // Insert with mixed case
        repo.sync_group_members(&group1.id, &["Alice@Co.com".into()])
            .await
            .unwrap();
        repo.sync_group_members(&group2.id, &["Alice@Co.com".into()])
            .await
            .unwrap();

        // Query with lowercase
        let mut groups = repo.find_groups_for_user("alice@co.com").await.unwrap();
        groups.sort();
        assert_eq!(groups, vec!["all@co.com", "eng@co.com"]);
    }
}
