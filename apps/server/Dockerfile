# Murmur (Mumble Server) for Railway
# Template-ready with environment variable configuration

FROM mumblevoip/mumble-server:latest

# The official image uses /data for persistence and configures via env vars
# Environment variables supported:
#   MUMBLE_SUPERUSER_PASSWORD - SuperUser password
#   MUMBLE_CONFIG_* - Any murmur.ini setting (e.g., MUMBLE_CONFIG_SERVER_PASSWORD)

# Murmur runs on port 64738 (TCP for Railway since UDP isn't supported)
EXPOSE 64738

# Volume for database persistence is configured via Railway Volumes (mounted at /data)
