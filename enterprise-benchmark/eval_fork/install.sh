#!/usr/bin/env bash
# Copy our chat-completions LLM client + patched factory into a local
# EnterpriseRAG-Bench checkout so the eval can be run with a non-OpenAI judge
# (DeepSeek, Moonshot, etc.).
#
# Usage:
#   ./install.sh /path/to/EnterpriseRAG-Bench
#
# Re-runnable; leaves a .bak of the original factory.py the first time.
set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "Usage: $0 /path/to/EnterpriseRAG-Bench" >&2
  exit 2
fi

target="$1"
src_dir="$(cd "$(dirname "$0")" && pwd)"

if [ ! -d "$target/src/llm" ]; then
  echo "ERROR: $target/src/llm not found — is this an EnterpriseRAG-Bench checkout?" >&2
  exit 1
fi

# Back up factory.py once
if [ ! -f "$target/src/llm/factory.py.bak" ]; then
  cp "$target/src/llm/factory.py" "$target/src/llm/factory.py.bak"
  echo "Backed up factory.py -> factory.py.bak"
fi

cp "$src_dir/chat_completions_llm.py" "$target/src/llm/chat_completions_llm.py"
cp "$src_dir/factory.py"               "$target/src/llm/factory.py"
echo "Installed chat_completions_llm.py + patched factory.py into $target/src/llm/"

echo
echo "To use:"
echo "  export LLM_PROVIDER=openai_compat"
echo "  export LLM_API_KEY=<deepseek-or-moonshot-or-other-key>"
echo "  export LLM_BASE_URL=https://api.deepseek.com/v1   # or https://api.moonshot.ai/v1"
echo "  export LLM_MODEL_NAME=deepseek-chat               # or kimi-k2.6"
echo "  python -m src.scripts.answer_evaluation.metrics_based_eval ..."
