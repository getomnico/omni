from evaluation.config import EvalConfig


def test_eval_config_has_chat_loop_defaults():
    cfg = EvalConfig()
    assert cfg.eval_chat_id == "eval-chat"
    assert cfg.eval_user_id == "eval-user"
    assert cfg.max_iterations == 10


def test_eval_config_from_env_reads_chat_loop_overrides(monkeypatch):
    monkeypatch.setenv("EVAL_CHAT_ID", "abc")
    monkeypatch.setenv("EVAL_USER_ID", "xyz")
    monkeypatch.setenv("EVAL_MAX_ITERATIONS", "3")
    cfg = EvalConfig.from_env()
    assert cfg.eval_chat_id == "abc"
    assert cfg.eval_user_id == "xyz"
    assert cfg.max_iterations == 3
