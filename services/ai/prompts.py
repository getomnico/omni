SOURCE_DISPLAY_NAMES = {
    "google_drive": "Google Drive",
    "gmail": "Gmail",
    "confluence": "Confluence",
    "jira": "Jira",
    "slack": "Slack",
    "hubspot": "HubSpot",
    "fireflies": "Fireflies",
    "web": "Web",
    "local_files": "Files",
    "github": "GitHub",
    "notion": "Notion",
    "onedrive": "OneDrive",
    "sharepoint": "SharePoint",
    "outlook": "Outlook",
    "outlook_calendar": "Outlook Calendar",
}

CHAT_SYSTEM_PROMPT_TEMPLATE = """You are Omni AI, a workplace assistant that helps employees find information and complete tasks.

You have access to tools. The most important is search_documents, which searches the unified index of all connected apps.

Connected apps: {connected_apps}

When searching, you MUST:
1. Name the specific app before each tool call (e.g., "I'll look through your docs on Google Drive").
2. Use the sources parameter to scope the search to that app.

Keep preambles short — just name the app and make the tool call."""


def build_chat_system_prompt(sources: list[dict]) -> str:
    seen = set()
    display_names = []
    for source in sources:
        source_type = source["source_type"]
        if source_type not in seen:
            seen.add(source_type)
            name = SOURCE_DISPLAY_NAMES.get(source_type, source_type)
            display_names.append(name)

    connected_apps = ", ".join(display_names) if display_names else "None"
    return CHAT_SYSTEM_PROMPT_TEMPLATE.format(connected_apps=connected_apps)
