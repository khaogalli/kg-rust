# Khaogalli Backend

## Table of Contents

- [Installation](#installation)
- [Configuration](#configuration)
- [Usage](#usage)

## Installation

1. Clone the repository:

   ```sh
   git clone https://github.com/khaogalli/kg-rust
   cd kg-rust
   ```

2. Build the project:

   ```sh
   cargo build --release
   ```

3. Install the required dependencies (if needed):
   ```sh
   cargo install --path .
   ```

## Configuration

This application can be configured via command-line arguments or environment variables. Additionally, a `.env` file can be used to provide these settings.

### Configurable Parameters

| Parameter            | Description                                   | Environment Variable | Command-line Argument  | Default |
| -------------------- | --------------------------------------------- | -------------------- | ---------------------- | ------- |
| `database_url`       | Connection URL for the PostgreSQL server      | `DATABASE_URL`       | `--database-url`       |         |
| `hmac_key`           | HMAC secret key for signing JWT tokens        | `HMAC_KEY`           | `--hmac-key`           |         |
| `db_max_connections` | Maximum number of connections to the database | `DB_MAX_CONNECTIONS` | `--db-max-connections` | `10`    |
| `db_min_connections` | Minimum number of connections to the database | `DB_MIN_CONNECTIONS` | `--db-min-connections` | `0`     |

### Example `.env` File

To use a `.env` file, create a file named `.env` in the root directory with the following content:

```env
DATABASE_URL=postgres://user:password@localhost/database
HMAC_KEY=your_secret_hmac_key
DB_MAX_CONNECTIONS=10
DB_MIN_CONNECTIONS=0
```

## Usage

To run the application, use the following command:

```sh
cargo run --release -- --database-url <DATABASE_URL> --hmac-key <HMAC_KEY> --db-max-connections <MAX_CONNECTIONS> --db-min-connections <MIN_CONNECTIONS>
```

Alternatively, if you are using environment variables or a `.env` file, you can omit these parameters:

```sh
cargo run --release
```

For help with command-line options, run:

```sh
cargo run -- --help
```
