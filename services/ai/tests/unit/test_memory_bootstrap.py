"""Unit tests for memory.bootstrap.build_mem0_config."""
from unittest.mock import AsyncMock, MagicMock, patch

import pytest


def _stub_provider(provider_type="openai", model_name="gpt-4o-mini", api_key="sk-x"):
    p = MagicMock()
    p.provider_type = provider_type
    p.model_name = model_name
    p.client.api_key = api_key
    p.client.base_url = None
    return p


def _stub_embed_cfg(provider="openai", model="text-embedding-3-small",
                    dims=1536, api_key="sk-e", api_url=None):
    e = MagicMock()
    e.provider = provider
    e.model = model
    e.dimensions = dims
    e.api_key = api_key
    e.api_url = api_url
    return e


@pytest.mark.unit
class TestBuildMem0Config:
    @pytest.fixture
    def state(self):
        s = MagicMock()
        s.models = {"m1": _stub_provider(), "m2": _stub_provider(model_name="gpt-4o")}
        s.default_model_id = "m1"
        s.embedding_provider = MagicMock()
        return s

    async def test_picks_memory_llm_id_when_set(self, state):
        from memory.bootstrap import build_mem0_config

        repo = MagicMock()
        repo.get = AsyncMock(return_value={"value": "m2"})
        with patch("memory.bootstrap.ConfigurationRepository", return_value=repo), \
             patch("memory.bootstrap.get_embedding_config",
                   AsyncMock(return_value=_stub_embed_cfg())):
            cfg = await build_mem0_config(
                state,
                database_host="db", database_port=5432,
                database_name="omni",
                mem0ai_user="mem0ai", mem0ai_password="mem0ai",
            )
        assert cfg["llm"]["config"]["model"] == "gpt-4o"

    async def test_falls_back_to_default_model(self, state):
        from memory.bootstrap import build_mem0_config

        repo = MagicMock()
        repo.get = AsyncMock(return_value=None)
        with patch("memory.bootstrap.ConfigurationRepository", return_value=repo), \
             patch("memory.bootstrap.get_embedding_config",
                   AsyncMock(return_value=_stub_embed_cfg())):
            cfg = await build_mem0_config(
                state,
                database_host="db", database_port=5432,
                database_name="omni",
                mem0ai_user="mem0ai", mem0ai_password="mem0ai",
            )
        assert cfg["llm"]["config"]["model"] == "gpt-4o-mini"

    async def test_probes_embedding_dims_when_missing(self, state):
        from memory.bootstrap import build_mem0_config

        chunk = MagicMock()
        chunk.embedding = [0.0] * 768
        state.embedding_provider.generate_embeddings = AsyncMock(return_value=[chunk])

        repo = MagicMock()
        repo.get = AsyncMock(return_value=None)
        embed_cfg = _stub_embed_cfg(dims=None)
        with patch("memory.bootstrap.ConfigurationRepository", return_value=repo), \
             patch("memory.bootstrap.get_embedding_config",
                   AsyncMock(return_value=embed_cfg)):
            cfg = await build_mem0_config(
                state,
                database_host="db", database_port=5432,
                database_name="omni",
                mem0ai_user="mem0ai", mem0ai_password="mem0ai",
            )
        assert cfg["embedder"]["config"]["embedding_dims"] == 768

    async def test_collection_name_fingerprints_embedder(self, state):
        from memory.bootstrap import build_mem0_config

        repo = MagicMock()
        repo.get = AsyncMock(return_value=None)
        with patch("memory.bootstrap.ConfigurationRepository", return_value=repo), \
             patch("memory.bootstrap.get_embedding_config",
                   AsyncMock(return_value=_stub_embed_cfg())):
            cfg_a = await build_mem0_config(
                state, database_host="db", database_port=5432,
                database_name="omni", mem0ai_user="mem0ai", mem0ai_password="mem0ai",
            )

        repo2 = MagicMock()
        repo2.get = AsyncMock(return_value=None)
        with patch("memory.bootstrap.ConfigurationRepository", return_value=repo2), \
             patch("memory.bootstrap.get_embedding_config",
                   AsyncMock(return_value=_stub_embed_cfg(model="text-embedding-3-large", dims=3072))):
            cfg_b = await build_mem0_config(
                state, database_host="db", database_port=5432,
                database_name="omni", mem0ai_user="mem0ai", mem0ai_password="mem0ai",
            )

        name_a = cfg_a["vector_store"]["config"]["collection_name"]
        name_b = cfg_b["vector_store"]["config"]["collection_name"]
        assert name_a.startswith("mem0_memories_") and len(name_a) == len("mem0_memories_") + 12
        assert name_a != name_b

    async def test_uses_mem0ai_credentials(self, state):
        from memory.bootstrap import build_mem0_config

        repo = MagicMock()
        repo.get = AsyncMock(return_value=None)
        with patch("memory.bootstrap.ConfigurationRepository", return_value=repo), \
             patch("memory.bootstrap.get_embedding_config",
                   AsyncMock(return_value=_stub_embed_cfg())):
            cfg = await build_mem0_config(
                state, database_host="db", database_port=5432,
                database_name="omni", mem0ai_user="mem0ai", mem0ai_password="mem0ai-secret",
            )
        vs = cfg["vector_store"]["config"]
        assert vs["user"] == "mem0ai"
        assert vs["password"] == "mem0ai-secret"
        assert vs["dbname"] == "omni"
        assert vs["host"] == "db"
        assert vs["port"] == 5432
