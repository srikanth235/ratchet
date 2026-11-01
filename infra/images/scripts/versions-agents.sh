# shellcheck shell=bash
# shellcheck disable=SC2034
# Pins for the agent CLIs. Sourced only by 50-agent-clis.sh and the runner image's upload list:
# the bench image never receives this file. Same rules as versions.sh.

# Bump together with an adapter test run: the Claude Code and Codex adapters parse their
# stream-json output.
CLAUDE_CODE_VERSION=2.1.300
CLAUDE_CODE_INTEGRITY=sha512-EOOmbzTxKxiukfV8T7K2+uDIxt8kR1vn+hgLB464HdjH1fZWS+qAc/nY6GYThcbe4kTqQOCFnyOcFKMXOxl+PQ==
CODEX_VERSION=0.162.1
CODEX_INTEGRITY=sha512-NWZdi/kxyjv/8EUGFupziGU38YyleugZRM4JXgY5XFH7FUmaFA33NZS2Bmq0HPazf7S3jJQQWsZ/jAK9jsrV3Q==
