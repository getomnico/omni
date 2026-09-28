from __future__ import annotations

from collections.abc import Mapping

from httpx import Response


def mock_effective_actions(router, handler, source_types: dict[str, str] | None = None) -> None:
    def response(_request):
        sources = handler._prefetched_sources or []
        source_records = [
            (source.id, source.source_type, source.integration_type, source.config)
            for source in sources
        ]
        known_ids = {source_id for source_id, _, _, _ in source_records}
        for source_id, source_type in (source_types or {}).items():
            if source_id in known_ids:
                continue
            for connector in handler.connector_catalog or []:
                manifest = connector.get("manifest")
                if connector.get("source_type") == source_type and isinstance(manifest, Mapping):
                    source_records.append(
                        (
                            source_id,
                            source_type,
                            manifest.get("integration_type", "connector"),
                            {},
                        )
                    )
        results = []
        for source_id, source_type, integration_type, source_config in source_records:
            for connector in handler.connector_catalog or []:
                if connector.get("source_type") != source_type:
                    continue
                manifest = connector.get("manifest")
                if not isinstance(manifest, Mapping):
                    continue
                if manifest.get("integration_type", "connector") != integration_type:
                    continue
                source_group = next(
                    (
                        group
                        for group in manifest.get("source_capabilities", [])
                        if group.get("source_id") == source_id
                    ),
                    None,
                )
                actions = list(source_group.get("actions", [])) if source_group else []
                names = {action.get("name") for action in actions}
                actions.extend(
                    action
                    for action in manifest.get("actions", [])
                    if action.get("name") not in names
                )
                for action in actions:
                    origin = action.get("origin", "native")
                    allowed = source_config.get("allowed_action_origins")
                    if allowed is not None and origin not in allowed:
                        continue
                    if action.get("hidden", False):
                        continue
                    results.append(
                        {
                            "source_id": source_id,
                            "source_type": source_type,
                            "name": action["name"],
                            "description": action.get("description", ""),
                            "input_schema": action.get(
                                "input_schema", {"type": "object", "properties": {}}
                            ),
                            "mode": action.get("mode", "write"),
                            "admin_only": action.get("admin_only", False),
                            "hidden": action.get("hidden", False),
                            "origin": origin,
                            "required_scopes": action.get("required_scopes"),
                            "actor_scoped": action.get("actor_scoped", False),
                            "supports_user_oauth": bool(manifest.get("oauth"))
                            and not action.get("admin_only", False)
                            and action.get("credential_scope", "user") != "org",
                        }
                    )
        return Response(200, json={"actions": results})

    return router.get("http://cm.test/actions").mock(side_effect=response)


def mock_ready_preflight(router, source_id: str, source_type: str) -> None:
    router.post("http://cm.test/actions/preflight").mock(
        return_value=Response(
            200,
            json={
                "state": "ready",
                "source_id": source_id,
                "source_type": source_type,
                "provider": None,
                "oauth_start_url": None,
                "missing_scopes": [],
            },
        )
    )
