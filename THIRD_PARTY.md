# Сторонние материалы

## Keyboop (macOS)

Этот проект — порт [Keyboop](https://github.com/iffuno/keyboop) на Linux. Логика детектора,
таблицы раскладок, правила чистки диктовки и `data/menubar-mark.png` перенесены оттуда.
Лицензия MIT, © 2026 Keyboop contributors — полный текст в [LICENSE](LICENSE).

## keyswitcher — языковые данные

Файлы `data/words_ru.json`, `data/words_en.json`, `data/trigrams_ru.json`,
`data/trigrams_en.json` взяты из проекта
[keyswitcher](https://github.com/graninilya/keyswitcher).

```
MIT License

Copyright (c) 2026 Ilya Granin

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Модели перевода OPUS-MT

Модели не входят в репозиторий: `tools/get_translate_models.sh` качает их с
[OPUS-MT](https://github.com/Helsinki-NLP/Opus-MT) и конвертирует в формат CTranslate2.

- `ru-en/opus-2020-02-26`, `en-ru/opus-2020-02-11` — © University of Helsinki
  (Language Technology Research Group), лицензия
  [CC-BY 4.0](https://creativecommons.org/licenses/by/4.0/). Изменения: конвертация в
  CTranslate2 с квантованием int8. Текст лицензии кладётся рядом с моделью (`LICENSE`).
- Tiedemann, J. & Thottingal, S. (2020). *OPUS-MT — Building open translation services for
  the World*. EAMT 2020.

Модели запускаются библиотекой [CTranslate2](https://github.com/OpenNMT/CTranslate2)
(MIT, © 2019 SYSTRAN) через привязки [ct2rs](https://github.com/jkawamoto/ctranslate2-rs) (MIT).

## Модели Whisper

Модели не входят в репозиторий: `keyboop download-model` качает их с
[ggerganov/whisper.cpp](https://huggingface.co/ggerganov/whisper.cpp) (MIT, исходные веса —
OpenAI Whisper, MIT).
