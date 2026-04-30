"""Unit tests for memory.providers.mem0.bootstrap.build_mem0_config."""

from unittest.mock import AsyncMock, MagicMock, patch

import pytest

from memory.providers.mem0.bootstrap import DatabaseSettings, build_mem0_config


def _stub_provider(provider_type="openai", model_name="gpt-4o-mini", api_key="sk-x"):
    p = MagicMock()
    p.provider_type = provider_type
    p.model_name = model_name
    p.api_key = api_key
    p.base_url = None
    return p


def _stub_embed_cfg(
    provider="openai",
    model="text-embedding-3-small",
    dims=1536,
    api_key="sk-e",
    api_url=None,
):
    e = MagicMock()
    e.provider = provider
    e.model = model
    e.dimensions = dims
    e.api_key = api_key
    e.api_url = api_url
    return e


_DB = DatabaseSettings(
    host="db", port=5432, dbname="omni", user="omni", password="omni"
)
_HISTORY = "/tmp/mem0_history.db"


def _patch_deps(repo_value, embed_cfg):
    repo = MagicMock()
    repo.get_global = AsyncMock(return_value=repo_value)
    return (
        patch(
            "memory.providers.mem0.bootstrap.ConfigurationRepository", return_value=repo
        ),
        patch(
            "memory.providers.mem0.bootstrap.get_embedding_config",
            AsyncMock(return_value=embed_cfg),
        ),
    )


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
        repo_patch, embed_patch = _patch_deps({"value": "m2"}, _stub_embed_cfg())
        with repo_patch, embed_patch:
            cfg = await build_mem0_config(state, db=_DB, history_db_path=_HISTORY)
        assert cfg.llm.config["model"] == "gpt-4o"

    async def test_falls_back_to_default_model(self, state):
        repo_patch, embed_patch = _patch_deps(None, _stub_embed_cfg())
        with repo_patch, embed_patch:
            cfg = await build_mem0_config(state, db=_DB, history_db_path=_HISTORY)
        assert cfg.llm.config["model"] == "gpt-4o-mini"

    async def test_probes_embedding_dims_when_missing(self, state):
        chunk = MagicMock()
        chunk.embedding = [0.0] * 768
        state.embedding_provider.generate_embeddings = AsyncMock(return_value=[chunk])

        repo_patch, embed_patch = _patch_deps(None, _stub_embed_cfg(dims=None))
        with repo_patch, embed_patch:
            cfg = await build_mem0_config(state, db=_DB, history_db_path=_HISTORY)
        assert cfg.embedder.config["embedding_dims"] == 768

    async def test_collection_name_fingerprints_embedder(self, state):
        repo_patch, embed_patch = _patch_deps(None, _stub_embed_cfg())
        with repo_patch, embed_patch:
            cfg_a = await build_mem0_config(state, db=_DB, history_db_path=_HISTORY)

        repo_patch_b, embed_patch_b = _patch_deps(
            None,
            _stub_embed_cfg(model="text-embedding-3-large", dims=3072),
        )
        with repo_patch_b, embed_patch_b:
            cfg_b = await build_mem0_config(state, db=_DB, history_db_path=_HISTORY)

        name_a = cfg_a.vector_store.config.collection_name
        name_b = cfg_b.vector_store.config.collection_name
        assert (
            name_a.startswith("mem0_memories_")
            and len(name_a) == len("mem0_memories_") + 12
        )
        assert name_a != name_b

    async def test_uses_main_omni_db_credentials(self, state):
        repo_patch, embed_patch = _patch_deps(None, _stub_embed_cfg())
        with repo_patch, embed_patch:
            cfg = await build_mem0_config(
                state,
                db=DatabaseSettings(
                    host="db",
                    port=5432,
                    dbname="omni",
                    user="omni",
                    password="omni-secret",
                ),
                history_db_path=_HISTORY,
            )
        pg = cfg.vector_store.config
        assert pg.user == "omni"
        assert pg.password == "omni-secret"
        assert pg.dbname == "omni"
        assert pg.host == "db"
        assert pg.port == 5432

    async def test_returns_typed_memory_config(self, state):
        from mem0.configs.base import MemoryConfig

        repo_patch, embed_patch = _patch_deps(None, _stub_embed_cfg())
        with repo_patch, embed_patch:
            cfg = await build_mem0_config(state, db=_DB, history_db_path=_HISTORY)

        assert isinstance(cfg, MemoryConfig)
        assert cfg.vector_store.provider == "pgvector"
        assert cfg.vector_store.config.user == "omni"
        assert cfg.llm.config["model"] == "gpt-4o-mini"
        assert cfg.embedder.config["model"] == "text-embedding-3-small"
