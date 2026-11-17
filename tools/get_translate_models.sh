#!/usr/bin/env bash
# Скачивает модели перевода OPUS-MT (Helsinki-NLP, CC-BY 4.0) RU→EN и EN→RU и конвертирует их
# в формат CTranslate2 (int8, ~80 МБ каждая) в ~/.local/share/keyboop/models/opus-mt-*.
#
# Нужен только python3: ctranslate2 ставится во временное окружение и удаляется в конце.
# PyTorch и transformers не нужны — у CTranslate2 свой конвертер для исходных моделей Marian.
# Скачивается ~550 МБ архивов (сервер OPUS бывает медленным).
set -euo pipefail

DEST="${XDG_DATA_HOME:-$HOME/.local/share}/keyboop/models"
BASE=https://object.pouta.csc.fi/OPUS-MT-models
MODELS=(
  "ru-en opus-2020-02-26"
  "en-ru opus-2020-02-11"
)

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

echo "→ ставлю ctranslate2 во временное окружение"
python3 -m venv "$WORK/venv"
"$WORK/venv/bin/pip" install -q --disable-pip-version-check ctranslate2

for m in "${MODELS[@]}"; do
  read -r pair release <<<"$m"
  out="$DEST/opus-mt-$pair"
  if [[ -f "$out/model.bin" ]]; then
    echo "✓ $pair уже есть: $out"
    continue
  fi
  echo "→ $pair: скачиваю $release.zip"
  curl -fL --retry 3 -C - -o "$WORK/$pair.zip" "$BASE/$pair/$release.zip"
  unzip -q "$WORK/$pair.zip" -d "$WORK/$pair"
  echo "→ $pair: конвертирую в int8"
  rm -rf "$out.tmp"
  "$WORK/venv/bin/ct2-opus-mt-converter" --model_dir "$WORK/$pair" --output_dir "$out.tmp" --quantization int8 >/dev/null
  cp "$WORK/$pair/source.spm" "$WORK/$pair/target.spm" "$WORK/$pair/LICENSE" "$out.tmp/"
  rm -rf "$out" && mv "$out.tmp" "$out"
  rm -rf "$WORK/$pair" "$WORK/$pair.zip"
  echo "✓ $pair: $out"
done

echo "Готово. Проверить: keyboop translate привет мир"
