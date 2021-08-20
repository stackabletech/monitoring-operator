#!/usr/bin/env sh
set -euo pipefail

VERSION_FILE=$(dirname  $(dirname $(dirname $0)))/VERSION
echo $NEW_VERSION > $VERSION_FILE

