#!/bin/bash
set -euo pipefail

echo $(pwd)

VERSION_FILE=$(dirname  $(dirname $(dirname $0)))/VERSION
echo $NEW_VERSION > $VERSION_FILE

