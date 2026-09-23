# txttsql-mcp

[English](README.md) · [Быстрый старт](QUICKSTART.md) · [Контракт плагинов](docs/operations/plugins.md) · [Безопасность](SECURITY.md)

MCP-сервер для аналитических запросов только на чтение к именованным источникам PostgreSQL, CockroachDB и ClickHouse. Плагины баз данных и метаданных устанавливаются отдельно, запускаются как исполняемые пакеты и имеют версионированные манифесты. Опционально доступны OpenMetadata, Airflow, локальная память успешных запросов и полученных метаданных.

Сервер предоставляет 13 MCP-инструментов: просмотр источников и таблиц, проверку и выполнение SQL, работу с онтологией, сохранёнными запросами и метаданными. У каждого источника свои разрешённые таблицы, лимиты строк и таймауты. SQL проверяется до вызова адаптера; PostgreSQL и CockroachDB используют транзакции только на чтение. **Дополнительно ограничьте права учётной записи в самой БД.**

## Быстрый старт

Нужны Rust 1.96+ и учётная запись БД с правами только на чтение.

```sh
git clone https://github.com/letya999/txttsql-mcp.git
cd txttsql-mcp
cp config.example.toml config.toml
# Заполните config.toml: ссылки на секреты env/file/broker и allowed_schemas либо allowed_tables.
cargo build --release --locked
APP_PG_PASSWORD='local-secret' ./target/release/txttsql-mcp --config config.toml
```

В Windows: `Copy-Item config.example.toml config.toml`, задайте `$env:APP_PG_PASSWORD`, запустите `.\target\release\txttsql-mcp.exe --config config.toml`. В примере конфигурации три источника: удалите или настройте каждый ненужный. Не коммитьте `config.toml` и файлы с секретами. Стандартный вывод занят протоколом MCP, поэтому для вызова инструментов подключите сервер к MCP-клиенту.

Для клиентов с форматом `mcpServers` возьмите [mcp.json](mcp.json), замените команду на абсолютный путь к бинарному файлу и путь к конфигурации. Приватный файл окружения можно загрузить параметром `--env-file PATH`. Начните с `list_sources`, `list_tables`, `describe_table`, `validate_sql`, затем `execute_sql`.

## Плагины и распространение

Плагины БД и метаданных подключаются отдельно через `plugin_databases` и `plugin_metadata` с путём к их `plugin.json`. В репозитории есть примеры SQLite, JSON-каталога и OpenMetadata. Плагины исполняются с правами текущего пользователя: устанавливайте только доверенные пакеты. Подробности: [плагины](docs/operations/plugins.md), [конфигурация](docs/operations/configuration.md), [быстрый старт](QUICKSTART.md).

Имя для MCP Registry: `mcp-name: io.github.letya999/txttsql-mcp`. [server.json](server.json) описывает будущий Cargo-пакет. Для регистрации в официальном MCP Registry сначала потребуется опубликовать версию 0.1.0 на crates.io; публикация исходников на GitHub этого не заменяет.

Лицензия: [GNU AGPL-3.0-only](LICENSE).
