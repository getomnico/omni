import ast
from pathlib import Path


def test_message_param_is_not_instantiated_anywhere():
    ai_root = Path(__file__).parents[2]
    constructor_calls = []

    for path in ai_root.walk(top_down=True):
        path, dirnames, filenames = path
        dirnames[:] = [
            name
            for name in dirnames
            if not name.startswith(".") and name not in {"__pycache__", "node_modules"}
        ]
        for filename in filenames:
            if not filename.endswith(".py"):
                continue
            source_path = path / filename
            tree = ast.parse(source_path.read_text())
            for node in ast.walk(tree):
                if (
                    isinstance(node, ast.Call)
                    and isinstance(node.func, ast.Name)
                    and node.func.id == "MessageParam"
                ):
                    constructor_calls.append(
                        f"{(path / filename).relative_to(ai_root)}:{node.lineno}"
                    )

    assert constructor_calls == []
