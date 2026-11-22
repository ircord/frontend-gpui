#!/bin/sh
# Run cargo in the build container, e.g. `bin/dev.sh clippy --all-targets`.
exec cargo xtask shell -- cargo "$@"
