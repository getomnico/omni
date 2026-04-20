import os
from dataclasses import dataclass


@dataclass
class EvalConfig:
    judge_model: str = "google/gemma-4-31b-it"
    judge_provider: str = "openai"

    faithfulness_threshold: float = 0.5
    context_recall_threshold: float = 0.4
    answer_relevancy_threshold: float = 0.5

    golden_set_path: str = "evaluation/datasets/golden_set.yaml"

    eval_chat_id: str = "eval-chat"
    eval_user_id: str = "eval-user"
    max_iterations: int = 10

    @classmethod
    def from_env(cls) -> "EvalConfig":
        return cls(
            judge_model=os.getenv("EVAL_MODEL", "google/gemma-4-31b-it"),
            judge_provider=os.getenv("EVAL_PROVIDER", "openai"),
            faithfulness_threshold=float(
                os.getenv("EVAL_FAITHFULNESS_THRESHOLD", "0.5")
            ),
            context_recall_threshold=float(
                os.getenv("EVAL_CONTEXT_RECALL_THRESHOLD", "0.4")
            ),
            answer_relevancy_threshold=float(
                os.getenv("EVAL_ANSWER_RELEVANCY_THRESHOLD", "0.5")
            ),
            eval_chat_id=os.getenv("EVAL_CHAT_ID", "eval-chat"),
            eval_user_id=os.getenv("EVAL_USER_ID", "eval-user"),
            max_iterations=int(os.getenv("EVAL_MAX_ITERATIONS", "10")),
        )
