"""GAQL query definitions for Google Ads non-metric sync."""

from __future__ import annotations

SYNC_QUERIES: dict[str, str] = {
    "customer": """
        SELECT
          customer.id,
          customer.descriptive_name,
          customer.currency_code,
          customer.time_zone,
          customer.manager,
          customer.test_account,
          customer.status,
          customer.resource_name
        FROM customer
    """,
    "campaign_budget": """
        SELECT
          campaign_budget.id,
          campaign_budget.name,
          campaign_budget.resource_name,
          campaign_budget.status,
          campaign_budget.delivery_method,
          campaign_budget.period,
          campaign_budget.explicitly_shared
        FROM campaign_budget
    """,
    "bidding_strategy": """
        SELECT
          bidding_strategy.id,
          bidding_strategy.name,
          bidding_strategy.resource_name,
          bidding_strategy.status,
          bidding_strategy.type,
          bidding_strategy.currency_code,
          bidding_strategy.campaign_count,
          bidding_strategy.non_removed_campaign_count
        FROM bidding_strategy
    """,
    "campaign": """
        SELECT
          campaign.id,
          campaign.name,
          campaign.resource_name,
          campaign.status,
          campaign.serving_status,
          campaign.advertising_channel_type,
          campaign.advertising_channel_sub_type,
          campaign.bidding_strategy_type,
          campaign.campaign_budget,
          campaign.labels
        FROM campaign
    """,
    "ad_group": """
        SELECT
          campaign.id,
          campaign.name,
          campaign.resource_name,
          ad_group.id,
          ad_group.name,
          ad_group.resource_name,
          ad_group.status,
          ad_group.type,
          ad_group.labels
        FROM ad_group
    """,
    "ad_group_ad": """
        SELECT
          campaign.id,
          campaign.name,
          campaign.resource_name,
          ad_group.id,
          ad_group.name,
          ad_group.resource_name,
          ad_group_ad.resource_name,
          ad_group_ad.status,
          ad_group_ad.ad.id,
          ad_group_ad.ad.name,
          ad_group_ad.ad.type,
          ad_group_ad.ad.final_urls,
          ad_group_ad.ad.display_url,
          ad_group_ad.ad.responsive_search_ad.headlines,
          ad_group_ad.ad.responsive_search_ad.descriptions
        FROM ad_group_ad
    """,
    "asset": """
        SELECT
          asset.id,
          asset.name,
          asset.resource_name,
          asset.type,
          asset.text_asset.text,
          asset.youtube_video_asset.youtube_video_id,
          asset.youtube_video_asset.youtube_video_title
        FROM asset
    """,
    "keyword_view": """
        SELECT
          campaign.id,
          campaign.name,
          campaign.resource_name,
          ad_group.id,
          ad_group.name,
          ad_group.resource_name,
          ad_group_criterion.criterion_id,
          ad_group_criterion.resource_name,
          ad_group_criterion.status,
          ad_group_criterion.negative,
          ad_group_criterion.type,
          ad_group_criterion.keyword.text,
          ad_group_criterion.keyword.match_type,
          ad_group_criterion.final_urls
        FROM keyword_view
    """,
    "shared_set": """
        SELECT
          shared_set.id,
          shared_set.name,
          shared_set.resource_name,
          shared_set.status,
          shared_set.type,
          shared_set.member_count,
          shared_set.reference_count
        FROM shared_set
    """,
    "user_list": """
        SELECT
          user_list.id,
          user_list.name,
          user_list.resource_name,
          user_list.description,
          user_list.membership_status,
          user_list.type,
          user_list.eligible_for_search,
          user_list.eligible_for_display
        FROM user_list
    """,
    "conversion_action": """
        SELECT
          conversion_action.id,
          conversion_action.name,
          conversion_action.resource_name,
          conversion_action.status,
          conversion_action.type,
          conversion_action.category,
          conversion_action.origin,
          conversion_action.counting_type,
          conversion_action.primary_for_goal,
          conversion_action.include_in_conversions_metric
        FROM conversion_action
    """,
    "recommendation": """
        SELECT
          recommendation.resource_name,
          recommendation.type,
          recommendation.campaign,
          recommendation.ad_group,
          recommendation.dismissed
        FROM recommendation
    """,
}

CHANGE_STATUS_QUERY_TEMPLATE = """
    SELECT
      change_status.resource_name,
      change_status.last_change_date_time,
      change_status.resource_type,
      change_status.campaign,
      change_status.ad_group,
      change_status.ad_group_ad,
      change_status.asset,
      change_status.campaign_budget,
      change_status.ad_group_criterion,
      change_status.user_list,
      change_status.conversion_action
    FROM change_status
    WHERE change_status.last_change_date_time > '{since}'
    ORDER BY change_status.last_change_date_time ASC
"""

REPORT_FIELD_ALLOWLIST = {
    "segments.date",
    "segments.week",
    "segments.month",
    "customer.id",
    "customer.descriptive_name",
    "campaign.id",
    "campaign.name",
    "campaign.status",
    "campaign.advertising_channel_type",
    "ad_group.id",
    "ad_group.name",
    "ad_group.status",
    "ad_group_ad.ad.id",
    "ad_group_ad.ad.name",
    "ad_group_criterion.criterion_id",
    "ad_group_criterion.keyword.text",
    "ad_group_criterion.keyword.match_type",
    "metrics.impressions",
    "metrics.clicks",
    "metrics.cost_micros",
    "metrics.ctr",
    "metrics.average_cpc",
    "metrics.conversions",
    "metrics.conversions_value",
    "metrics.all_conversions",
}

REPORT_RESOURCE_ALLOWLIST = {
    "customer",
    "customer_client",
    "campaign",
    "campaign_budget",
    "ad_group",
    "ad_group_ad",
    "ad_group_ad_asset_view",
    "keyword_view",
    "search_term_view",
    "expanded_landing_page_view",
    "change_event",
}
