docker compose -f docker/docker-compose.yml -f docker/docker-compose.dev.yml --env-file .env build
docker compose -f docker/docker-compose.yml -f docker/docker-compose.dev.yml --env-file .env up -d
docker compose -f docker/docker-compose.yml -f docker/docker-compose.dev.yml --env-file .env logs -f