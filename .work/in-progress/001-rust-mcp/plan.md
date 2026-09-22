# Реализация Rust MCP

1. Инвентаризовать `txttsql`, шаблон memory bank и первичные источники MCP.
2. Реализовать источник, секреты, SQL guard, DB adapter registry и stdio MCP.
3. Добавить опциональную память, онтологию, OpenMetadata и Airflow.
4. Проверить Cargo gates, протокол MCP, Docker и безопасность; исправить найденное.

Проверено 22–23.09.2026: `just ci`, HTTP-тесты OpenMetadata/Airflow v1/v2 с Bearer/Basic, независимые live-тесты PostgreSQL/CockroachDB/ClickHouse, одновременные MCP-вызовы всех трёх БД с каталогом таблиц и столбцов, чтение реального графа `txttsql/memory`, security scan/Opengrep и Docker build + stdio smoke (13 инструментов). После коммита `4d813ee` дополнительно проверен Airflow 2.10.5 в одноразовом контейнере: поиск DAG, задачи и связи. Проверен TLS с собственным CA: локальный HTTPS-сервер отклоняет подключение без PEM и принимает с ним, одноразовый PostgreSQL с TLS проходит `pgwire_live`. Интеграция OpenMetadata проверена локальным HTTP-сервером; публичный sandbox отвечает 401, поэтому live-проверка на самом сервисе возможна только с тестовым токеном.
