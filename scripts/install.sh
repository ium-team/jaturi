#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC_BIN="${SCRIPT_DIR}/jaturi"

if [[ ! -f "${SRC_BIN}" ]]; then
  echo "Error: jaturi binary not found next to install.sh"
  exit 1
fi

chmod +x "${SRC_BIN}"

TARGET_DIR="${HOME}/.local/bin"
TARGET_BIN="${TARGET_DIR}/jaturi"

mkdir -p "${TARGET_DIR}"
cp "${SRC_BIN}" "${TARGET_BIN}"
chmod +x "${TARGET_BIN}"

case ":${PATH}:" in
  *":${TARGET_DIR}:"*)
    PATH_ALREADY_SET=1
    ;;
  *)
    PATH_ALREADY_SET=0
    ;;
esac

ACTIVE_SHELL="$(basename "${SHELL:-}")"
if [[ "${ACTIVE_SHELL}" == "zsh" ]]; then
  PROFILE_FILE="${ZDOTDIR:-${HOME}}/.zshrc"
elif [[ "${ACTIVE_SHELL}" == "bash" ]]; then
  PROFILE_FILE="${HOME}/.bashrc"
else
  PROFILE_FILE="${HOME}/.profile"
fi

mkdir -p "$(dirname "${PROFILE_FILE}")"
touch "${PROFILE_FILE}"
if ! grep -Fq 'export PATH="$HOME/.local/bin:$PATH"' "${PROFILE_FILE}"; then
  {
    echo
    echo '# Added by jaturi installer'
    echo 'export PATH="$HOME/.local/bin:$PATH"'
  } >> "${PROFILE_FILE}"
fi

echo "Installed to ${TARGET_BIN}"
echo "PATH persistence ensured in ${PROFILE_FILE}"
if [[ ${PATH_ALREADY_SET} -eq 0 ]]; then
  echo "Open a new terminal or run: export PATH=\"$HOME/.local/bin:$PATH\""
fi
echo "Run: jaturi"
