"""Configuration constants for HubSpot connector."""

# Batch size for pagination (HubSpot max is 100)
BATCH_SIZE = 100

# Object types to sync
HUBSPOT_OBJECT_TYPES = [
    # CRM Core Objects
    "contacts",
    "companies",
    "deals",
    "tickets",
    # CRM Activities (Engagements)
    "calls",
    "emails",
    "meetings",
    "notes",
    "tasks",
]

# Properties to fetch for each object type
HUBSPOT_OBJECT_CONFIGS: dict[str, dict[str, list[str]]] = {
    "contacts": {
        "title_fields": [],  # Use _contact_title fallback for proper name combination
        "properties": [
            "firstname",
            "lastname",
            "email",
            "phone",
            "company",
            "jobtitle",
            "lifecyclestage",
            "createdate",
            "hs_lastmodifieddate",
            "hubspot_owner_id",
        ],
    },
    "companies": {
        "title_fields": ["name"],
        "properties": [
            "name",
            "domain",
            "industry",
            "phone",
            "numberofemployees",
            "annualrevenue",
            "createdate",
            "hs_lastmodifieddate",
            "hubspot_owner_id",
        ],
    },
    "deals": {
        "title_fields": ["dealname"],
        "properties": [
            "dealname",
            "amount",
            "pipeline",
            "dealstage",
            "closedate",
            "hs_deal_stage_probability",
            "createdate",
            "hs_lastmodifieddate",
            "hubspot_owner_id",
        ],
    },
    "tickets": {
        "title_fields": ["subject"],
        "properties": [
            "subject",
            "content",
            "hs_ticket_priority",
            "hs_pipeline",
            "hs_pipeline_stage",
            "createdate",
            "hs_lastmodifieddate",
            "hubspot_owner_id",
        ],
    },
    "calls": {
        "title_fields": ["hs_call_title"],
        "properties": [
            "hs_call_title",
            "hs_call_body",
            "hs_call_duration",
            "hs_call_direction",
            "hs_timestamp",
            "hs_createdate",
            "hs_lastmodifieddate",
            "hubspot_owner_id",
        ],
    },
    "emails": {
        "title_fields": ["hs_email_subject"],
        "properties": [
            "hs_email_subject",
            "hs_email_text",
            "hs_email_html",
            "hs_email_direction",
            "hs_timestamp",
            "hs_createdate",
            "hs_lastmodifieddate",
            "hubspot_owner_id",
        ],
    },
    "meetings": {
        "title_fields": ["hs_meeting_title"],
        "properties": [
            "hs_meeting_title",
            "hs_meeting_body",
            "hs_meeting_start_time",
            "hs_meeting_end_time",
            "hs_timestamp",
            "hs_createdate",
            "hs_lastmodifieddate",
            "hubspot_owner_id",
        ],
    },
    "notes": {
        "title_fields": [],
        "properties": [
            "hs_note_body",
            "hs_timestamp",
            "hs_createdate",
            "hs_lastmodifieddate",
            "hubspot_owner_id",
        ],
    },
    "tasks": {
        "title_fields": ["hs_task_subject"],
        "properties": [
            "hs_task_subject",
            "hs_task_body",
            "hs_task_status",
            "hs_task_priority",
            "hs_timestamp",
            "hs_createdate",
            "hs_lastmodifieddate",
            "hubspot_owner_id",
        ],
    },
}
