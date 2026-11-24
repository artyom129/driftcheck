<div align="center">

# driftcheck

**Проверяет миграцию данных запись за записью и говорит, можно ли её принять. В 159 раз быстрее Python-инструмента, с которым сверяется, и с теми же ответами.**

[English](README.md) · [Русский](README.ru.md) · [Қазақша](README.kk.md)

![Rust](https://img.shields.io/badge/Rust-000000?logo=rust&logoColor=white)
![Version](https://img.shields.io/badge/version-0.2.0-2F6FEB)
![Tests](https://img.shields.io/badge/tests-40%20passed-2E8B57)
![License](https://img.shields.io/badge/license-MIT-lightgrey)

</div>

## Что делает

Вы перенесли клиентов, заказы или поставщиков из одной системы в другую. С обеих сторон по 120 000 строк. driftcheck скажет, те же ли это 120 000 записей с теми же значениями, и перечислит каждую, которая отличается.

```text
$ driftcheck check --source legacy.csv --target migrated.csv --contract mapping.json --csv findings.csv
driftcheck  legacy.csv -> migrated.csv
120300 source rows, 118700 target rows, loaded in 185 ms, reconciled in 2007 ms

  matched            107000
  fuzzy_matched        4000
  changed              6500
  source_only          2800
  target_only          1200
  ambiguous               0

  changed fields       6500
    plan -> tier                          3000    2.55%
    mrr -> monthly_revenue                2000     1.7%
    start_date -> activated_at            1500    1.28%
  repeated source keys: 300

quality 91.4  rejected
  - 6500 changed records exceeds allowed maximum 0
  - 2800 source-only records exceeds allowed maximum 0
  - 1200 target-only records exceeds allowed maximum 0
  - quality score 91.4 is below required 99.0
```

| Исход | Что значит |
|---|---|
| `matched` | Ключ совпал, все поля маппинга согласны |
| `fuzzy_matched` | Ключ сменился, запись найдена по имени и email |
| `changed` | Запись найдена, но поле расходится сверх правила, оба значения в отчёте |
| `source_only` | Есть в старой системе, нет в новой |
| `target_only` | Есть в новой системе, из старой на неё ничего не указывает |
| `ambiguous` | Два кандидата набрали почти одинаково, решать человеку |

Форматирование различием не считается. Email в верхнем регистре, пробелы вокруг имени, `1.290,00` против `1290` и `02.03.2026` против `2026-03-02` совпадают.

## Что нового в этой версии (0.2)

Версия 0.1 отвечала на один вопрос: можно ли принять миграцию. Версия 0.2 закрывает и то, что происходит до проверки и после неё. Контракт больше не нужно писать руками, отчёт показывает, в какой колонке проблема, а на выходе есть SQL, который приводит цель в порядок.

| Что | Как теперь работает |
|---|---|
| `driftcheck init` | Составляет контракт маппинга по двум файлам. Колонки сопоставляются по содержимому, поэтому `full_name` находит `name`, а `mrr` находит `monthly_revenue`, хотя названия разные. Так же выбираются ключ, правило для каждой пары и поля для поиска переименованных записей. На 120 000 строк черновик готов за 1,7 с и даёт тот же вердикт, что контракт, написанный вручную |
| Разбивка по полям | Сводка и JSON-отчёт перечисляют каждую расходящуюся колонку, худшую первой, с числом и долей: `plan -> tier  3000  2.55%`. Сразу видно, что сломан маппинг тарифа, а не 6 500 разных ошибок |
| SQL-план исправлений | `--patch fix.sql --table crm.customers` пишет одну транзакцию: `UPDATE` только расходящихся полей для каждой изменённой записи, `INSERT` для каждой пропавшей и закомментированный `DELETE` для каждой лишней, чтобы удаление решал человек. Ключи, повторённые в источнике, пропускаются с пометкой. Кавычки в значениях и именах экранируются |
| Отчёт в Markdown | `--markdown "$GITHUB_STEP_SUMMARY"` выводит вердикт, счётчики, таблицу по полям и первые 25 находок на страницу задачи GitHub Actions. Файл дописывается, поэтому несколько проверок делят одну сводку |
| Любой разделитель | Файлы с `;`, табуляцией или `\|` загружаются без параметров. Разделитель берётся из строки заголовков, а `--delimiter` задаёт его явно |

Скорость сверки та же, что в 0.1: 120 000 строк примерно за 2,5 с на том же ноутбуке.

```bash
driftcheck init  --source legacy.csv --target migrated.csv              # пишет mapping.json
driftcheck check --source legacy.csv --target migrated.csv --contract mapping.json \
  --patch fix.sql --table crm.customers --markdown summary.md
```

```text
driftcheck init  legacy.csv -> migrated.csv

  key  customer_id -> id
       full_name              -> name                   text               values 100.0%
       email                  -> email_address          email              values  99.0%
       plan                   -> tier                   text               values 100.0%
       mrr                    -> monthly_revenue        numeric_tolerance  values 100.0%
       start_date             -> activated_at           date               values 100.0%
       country                -> country_code           text               values 100.0%
  renamed records are searched by: full_name, email
```

```sql
BEGIN;
UPDATE "crm"."customers" SET "tier" = 'Business' WHERE "id" = 'C-100005';
UPDATE "crm"."customers" SET "monthly_revenue" = '299.00' WHERE "id" = 'C-100090';
INSERT INTO "crm"."customers" ("id", "name", "email_address", "tier", "monthly_revenue", "activated_at", "country_code")
  VALUES ('C-100127', 'Mason Tanaka', '9x6oyh3cvdo3@example.com', 'Business', '299.00', '2025-04-19', 'ES');
-- DELETE FROM "crm"."customers" WHERE "id" = 'C-901149';
COMMIT;
-- 6500 updates, 2500 inserts, 1200 deletes to review, 300 skipped
```

## Скорость

Одни и те же сгенерированные миграции, один контракт маппинга, один ноутбук (Intel i7-3612QM, 4 ядра, 8 потоков, 2012 год). Время вместе с чтением файлов.

| Строк | SyncProof (Python) | driftcheck, 8 потоков | driftcheck, 1 поток |
|---:|---:|---:|---:|
| 12 000 | 5,28 с | 0,23 с | 0,28 с |
| 120 000 | 393,8 с | 2,47 с | 5,92 с |

На обоих размерах инструменты дали одинаковые числа. На 120 000 строк это 107 000 совпавших, 4 000 переименованных, 6 500 изменённых, 2 800 пропавших и 1 200 лишних.

Больше всего времени уходит на записи со сменившимся ключом: каждую сравнивают со всеми свободными строками цели. driftcheck делает это битово-параллельным алгоритмом, который обрабатывает 64 символа за одно машинное слово, и раскладывает кандидатов по всем ядрам.

Повторить замер:

```bash
cargo build --release
./target/release/driftcheck gen --rows 120000 --out bench/data/120000
./target/release/driftcheck check --source bench/data/120000/legacy.csv --target bench/data/120000/migrated.csv --contract bench/data/120000/mapping.json
python bench/syncproof_bench.py ../syncproof bench/data/120000   # нужен клон SyncProof с его venv
```

## Как проходит проверка

```mermaid
flowchart LR
    A["legacy.csv"] --> L["Параллельная загрузка, разделитель определяется сам"]
    B["migrated.csv"] --> L
    I["driftcheck init"] -.-> C["mapping.json"]
    C --> K
    L --> K["Проход 1: точные ключи"]
    K --> F["Проход 2: переименованные записи, все ядра"]
    F --> D["Правила полей и допуски"]
    D --> G["Пределы приёмки"]
    G --> V["Вердикт и код выхода"]
    G --> R["JSON, CSV, Markdown"]
    G --> P["SQL-план исправлений"]
```

Сначала занимаются точные ключи, поэтому нечёткая догадка не может отнять строку у другого ключа, а результат не зависит от порядка строк.

## Быстрый старт

```bash
cargo install --path .
driftcheck gen --rows 1200 --out demo           # миграция с известными повреждениями
driftcheck init --source demo/legacy.csv --target demo/migrated.csv --out demo/drafted.json
driftcheck check --source demo/legacy.csv --target demo/migrated.csv \
  --contract demo/drafted.json --json report.json --csv findings.csv --patch fix.sql
echo $?                                         # 0 принято, 1 отклонено, 2 ошибка
```

| Параметр `check` | Назначение |
|---|---|
| `--json report.json` | Сводка в JSON с разбивкой по полям. С `--records` туда попадает каждая запись |
| `--csv findings.csv` | Строка на каждое различие и каждую запись без пары, файл безопасно открывать в Excel |
| `--all` | Включить в CSV и чистые совпадения по ключу |
| `--markdown summary.md` | Дописать сводку в Markdown, например в `$GITHUB_STEP_SUMMARY` |
| `--patch fix.sql` | Записать SQL-план исправлений. `--table schema.table` задаёт таблицу цели |
| `--delimiter semicolon` | Задать разделитель вместо автоопределения: символ или `comma`, `semicolon`, `tab`, `pipe` |
| `--threads N` | Ограничить число потоков |
| `--quiet` | Ничего не печатать, смотреть на код выхода и отчёты |

`init` по умолчанию пишет `mapping.json`, не перезаписывает существующий файл без `--force` и печатает каждую пару с долей общих значений. Колонки без пары перечисляются, так что ничего не теряется молча.

## Контракт маппинга

Тот же JSON, что читает [SyncProof](https://github.com/artyom129/syncproof), поэтому один контракт работает в обоих инструментах.

```json
{
  "source_key": "customer_id",
  "target_key": "id",
  "mappings": [
    {"source": "full_name",  "target": "name",            "rule": "text"},
    {"source": "email",      "target": "email_address",   "rule": "email"},
    {"source": "mrr",        "target": "monthly_revenue", "rule": "numeric_tolerance", "tolerance": 0.01},
    {"source": "start_date", "target": "activated_at",    "rule": "date"}
  ],
  "fuzzy_fields": [
    {"source": "full_name", "target": "name"},
    {"source": "email",     "target": "email_address"}
  ],
  "fuzzy_threshold": 90,
  "acceptance": {"max_changed": 0, "max_source_only": 0, "max_target_only": 0, "min_quality_score": 99}
}
```

| Правило | Что сравнивает |
|---|---|
| `exact` | Значения как есть |
| `text` | Текст без пробелов по краям, регистра и повторных пробелов |
| `email` | Адреса без учёта регистра |
| `numeric` | Числа: `1,234.50`, `1.234,50` и `$1,234.5` совпадают |
| `numeric_tolerance` | Числа в пределах `tolerance` |
| `date` | ISO, `YYYY/MM/DD`, `MM/DD/YYYY` и `DD.MM.YYYY`, `tolerance` в днях |

## В конвейере

Код выхода превращает проверку в барьер: задача миграции, оставившая повреждения, роняет сборку.

```yaml
- name: Migration acceptance
  run: >
    driftcheck check --source export/legacy.csv --target export/new.csv --contract mapping.json
    --csv findings.csv --patch fix.sql --markdown "$GITHUB_STEP_SUMMARY"
```

Вердикт и таблица по полям появляются на странице задачи, а план исправлений можно сохранить артефактом сборки для проверки.

## Тесты

```bash
cargo test --release
```

40 тестов. Главный прогоняет собственный тестовый набор SyncProof на 1 200 клиентов и требует тех же чисел, той же оценки качества 91.4 и тех же четырёх причин отказа слово в слово. Другой генерирует 20 000 строк и требует найти каждое заложенное повреждение. Тесты `init` требуют, чтобы черновик контракта сопоставил все семь колонок на 1 200 и на 30 000 перемешанных строк и чтобы проверка с ним нашла каждое заложенное повреждение. Тесты SQL-плана проверяют экранирование и значения с переводом строки. CI запускается на Linux и Windows, составляет контракт, проверяет по нему и пишет сводку на страницу задачи.

## Структура проекта

```text
src/
  config.rs     контракт маппинга
  data.rs       загрузка CSV и определение разделителя
  value.rs      правила сравнения: текст, числа, даты
  fuzzy.rs      битово-параллельная похожесть для переименованных записей
  reconcile.rs  два прохода, исходы, разбивка по полям и вердикт
  infer.rs      driftcheck init: контракт по данным
  patch.rs      SQL-план исправлений
  report.rs     сводка в терминале, JSON, CSV и Markdown
  gen.rs        тестовые миграции любого размера с известными повреждениями
  main.rs       командная строка
tests/compat.rs тестовый набор SyncProof и прогон на 20 000 строк
bench/          скрипт замера SyncProof
```

## Версии

| Версия | Что принесла |
|---|---|
| **0.2.0** | `driftcheck init` составляет контракт по данным. Разбивка по полям в терминале и JSON. SQL-план исправлений через `--patch`. Сводка в Markdown через `--markdown`. Определение разделителя и `--delimiter`. 40 тестов |
| 0.1.0 | Сверка запись за записью, совместимая с контрактами SyncProof: два прохода, шесть исходов, пределы приёмки, отчёты JSON и CSV, коды выхода, генератор тестовых данных. В 159 раз быстрее SyncProof на 120 000 строк. 26 тестов |

---

<div align="center">

Автор: **[Koncha Artyom](https://github.com/artyom129)** · Лицензия MIT

</div>
