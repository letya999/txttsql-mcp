# Подключаемые плагины

Плагины БД и метаданных — отдельные каталоги с `plugin.json` и исполняемой программой. Их можно разрабатывать, версионировать и устанавливать независимо от Rust-сервера. Сервер запускает только явно перечисленные в конфигурации плагины. Примеры: [`plugins/sqlite`](../../plugins/sqlite/), [`plugins/catalog-json`](../../plugins/catalog-json/) и [`plugins/openmetadata`](../../plugins/openmetadata/). Встроенные PostgreSQL, CockroachDB, ClickHouse, OpenMetadata и Airflow продолжают работать без плагинов.

## Манифест и конфигурация

Манифест БД:

```json
{
  "manifest_version": 1,
  "id": "sqlite",
  "version": "0.1.0",
  "kind": "database",
  "dialect": "ansi",
  "command": "python3",
  "command_windows": "python",
  "args": ["plugin.py"],
  "capabilities": ["execute", "list_tables", "describe_table"]
}
```

Для каталога укажите `kind: "metadata"`, уберите `dialect` и объявите `search`, `detail`. `manifest_version` задаёт версию протокола, `version` — версию пакета. Сейчас поддерживается только протокол 1. Для БД допустимы диалекты `postgres`, `clickhouse`, `ansi`; новый диалект требует поддержки в SQL guard сервера. Команда запускается напрямую, без shell. Имя программы без разделителей ищется в `PATH`; путь вида `./bin/plugin` разрешается внутри каталога пакета. `args` передаются буквально, рабочий каталог — каталог манифеста. `command_windows` необязателен.

Пример `config.toml` с двумя независимыми экземплярами:

```toml
[[plugin_databases]]
id = "local_metrics"
manifest = "/opt/txttsql/plugins/sqlite/plugin.json"
allowed_schemas = ["main"]
max_rows = 500
workers = 2
timeout_seconds = 30
[plugin_databases.settings]
path = "/data/metrics.sqlite"

[[plugin_metadata]]
id = "local_catalog"
manifest = "/opt/txttsql/plugins/catalog-json/plugin.json"
timeout_seconds = 30
[plugin_metadata.settings]
path = "/data/catalog.json"
```

`id` экземпляра не обязан совпадать с `id` пакета: один пакет можно подключить несколько раз с разными `settings`, `secrets`, allowlist и лимитами. Секреты задаются только ссылками `env`, `file` или `broker`, например `[plugin_metadata.secrets] token = { kind = "env", name = "OPENMETADATA_TOKEN" }`. Не помещайте токен в `settings`. Для БД задайте `allowed_schemas` и/или `allowed_tables`; пустой allowlist запрещает чтение таблиц. `manifest` относительно файла конфигурации или абсолютный. Пакет и его зависимости должны присутствовать в среде запуска. Плагины запускаются с правами того же системного пользователя, что и MCP-сервер.

Можно настроить только провайдеры метаданных без источника БД. Тогда инструменты метаданных доступны, а SQL-инструменты не найдут источник до его добавления.

Базовый Docker-образ не содержит Python. Для трёх поставляемых примеров соберите образ с ним и пакетами:

```sh
docker build -t txttsql-mcp:local .
docker build -f Dockerfile.plugins -t txttsql-mcp:plugins .
```

В конфигурации внутри контейнера используйте пути `/opt/txttsql/plugins/.../plugin.json`. Сам `config.toml` и данные примонтируйте отдельно с нужными правами чтения. Собственные пакеты можно монтировать независимо в другой каталог или собирать их в отдельный образ.

## Контракт `txttsql-plugin/1`

Обмен идёт по stdin/stdout: один JSON-объект на строку, UTF-8, без диагностических сообщений в stdout. Сервер первым отправляет:

```json
{"protocol":1,"id":1,"method":"hello","payload":{"plugin":"sqlite","kind":"database"}}
```

Плагин отвечает тем же `id`, своей идентичностью и точным набором объявленных capabilities:

```json
{"protocol":1,"id":1,"ok":true,"result":{"plugin":"sqlite","kind":"database","capabilities":["execute","list_tables","describe_table"]}}
```

Неверный ответ, версия, `id`, вид плагина или capabilities отклоняются. Для остальных вызовов `payload` имеет вид `{"instance":"local_metrics","settings":{...},"secrets":{...},"params":{...}}`. Секреты разрешаются сервером заново перед каждым вызовом и передаются только через stdin дочернего процесса. Успех: `{"protocol":1,"id":2,"ok":true,"result":{...}}`; ошибка: `{"protocol":1,"id":2,"ok":false,"error":"..."}`. Не записывайте секреты или содержимое запросов в stderr.

| Вид | Метод и `params` | Обязательный `result` |
| --- | --- | --- |
| БД | `execute`: `{ "sql": "SELECT ...", "row_cap": 500, "tables": ["main.metrics"] }` | `{ "rows": [{...}], "truncated": false, "elapsed_ms": 12 }` |
| БД | `list_tables`: `{}` | `{ "tables": ["main.metrics"] }` |
| БД | `describe_table`: `{ "table": "main.metrics" }` | `{ "columns": [{"column_name":"id","data_type":"INTEGER"}], "truncated": false }` |
| Метаданные | `search`: `{ "text": "payments" }` | Любой JSON-каталог, например `{ "items": [{"id":"payments"}] }` |
| Метаданные | `detail`: `{ "id": "payments" }` | Любая JSON-карточка метаданных |

Сервер проверяет SQL, allowlist и лимит строк до `execute`, затем проверяет размер результата и число строк. Имена таблиц из `list_tables` дополнительно фильтруются allowlist. Плагин БД всё равно обязан открыть соединение с read-only правами и ограничить выполнение на стороне движка; объявленный диалект сам по себе не является песочницей. Для метаданных сервер ограничивает ответ 4 МБ, для любого сообщения — 16 МБ, для запроса к плагину — 256 КБ. Процесс остаётся запущенным для следующих вызовов; при ошибке транспорта или таймауте сервер уничтожает его и запускает новый при следующем вызове. `workers` задаёт число процессов БД; у каталога один процесс.

Для подключения пакета OpenMetadata задайте `[[plugin_metadata]]` с `manifest` на `plugins/openmetadata/plugin.json`, `[plugin_metadata.settings] url = "https://..."` и `[plugin_metadata.secrets] token = { kind = "env", name = "OPENMETADATA_TOKEN" }`. Для внутреннего CA доступен `ca_file`. `allow_insecure = true` допускает HTTP только в изолированном тесте. Этот пакет использует маршруты `/api/v1/search/query` и `/api/v1/tables/name/{fqn}` и возвращает JSON OpenMetadata без преобразования.

Для проверки пакета используйте `cargo test --locked --test mcp_protocol loads_independent_database_and_metadata_plugins`: тест запускает все три поставляемых манифеста без внешних сервисов. Он проверяет контракт и защиту на локальных одноразовых данных, а также маршруты и авторизацию OpenMetadata через локальный HTTP-макет. Совместимость с реальным OpenMetadata этим тестом не подтверждается; для неё нужен отдельный read-only тестовый сервис.
