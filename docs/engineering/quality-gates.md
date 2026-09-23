# Проверки

`just ci` запускает форматирование, `cargo check --locked`, строгий Clippy и тесты. GitHub Actions выполняет те же Rust-проверки на Linux, проверяет отсутствие секретных файлов в Git, собирает Docker-образ и проверяет запись SQLite в новый Docker volume от непривилегированного пользователя. Обычный `cargo test --locked` покрывает SQL guard, конфигурацию секретов, память, графы, HTTP-маршруты OpenMetadata/Airflow и MCP stdio. Сетевые проверки помечены `#[ignore]`: их запускают только на явно созданных одноразовых PostgreSQL, CockroachDB и ClickHouse.

Для `tests/pgwire_live.rs` задайте `PGWIRE_TEST_KIND=postgres|cockroach`, `PGWIRE_TEST_PORT`, `PGWIRE_TEST_USER`, `PGWIRE_TEST_PASSWORD` и запустите `cargo test --locked --test pgwire_live -- --ignored`. Для ClickHouse нужны `CH_TEST_PORT`, `CH_TEST_PASSWORD` и `cargo test --locked --test clickhouse_live -- --ignored`. Для проверки трёх БД в одном MCP процессе задайте `MIXED_TEST_PG_PORT`, `MIXED_TEST_CR_PORT`, `MIXED_TEST_CH_PORT`, `MIXED_TEST_PASSWORD` и запустите `cargo test --locked --test mcp_protocol one_mcp_process_serves_three_databases -- --ignored`. Все тестовые БД должны содержать `analytics.public.metrics` и `analytics.public.other` (ClickHouse: `analytics.metrics`, `analytics.other`).

Для TLS-проверки PostgreSQL/CockroachDB с внутренним CA дополнительно задайте `PGWIRE_TEST_CA_FILE` (PEM).

Для проверки существующей онтологии без копирования данных задайте `ONTOLOGY_TEST_ROOT` на каталог `txttsql/memory` и запустите `cargo test --locked --test ontology_live -- --ignored`.

Для отдельного тестового Airflow 2 задайте `AIRFLOW_TEST_URL`, `AIRFLOW_TEST_USER`, `AIRFLOW_TEST_PASSWORD`, `AIRFLOW_TEST_DAG` и запустите `cargo test --locked --test airflow_live -- --ignored`. Тест проверяет реальный REST API v1, поиск DAG и чтение списка задач; он не меняет DAG.

Для тестового OpenMetadata задайте `OPENMETADATA_TEST_URL`, `OPENMETADATA_TEST_TOKEN`, `OPENMETADATA_TEST_TABLE` (полное имя таблицы) и запустите `cargo test --locked --test openmetadata_live -- --ignored`. Публичный sandbox требует авторизации; тест не запускается без отдельного тестового токена.

Перед коммитом: `git diff --check`, скан секретов и проверка зависимостей. Настоящие пароли и рабочие базы в CI запрещены. `osv-scanner scan -L Cargo.lock` сейчас сообщает `RUSTSEC-2023-0071` для `rsa 0.9.10`: этот пакет включён в lockfile как необязательная зависимость SQLx MySQL, но `cargo tree -i rsa --target all -e all` показывает, что в активном графе данного сервера его нет. Предупреждение перепроверяют при изменении зависимостей.
