#!/bin/bash
set -e

MURMUR_LOG_FILE="${MURMUR_LOG_FILE:-/data/murmur.log}"
MURMUR_STREAM_LOGS="${MURMUR_STREAM_LOGS:-1}"
MURMUR_VERBOSE_LEVEL="${MURMUR_VERBOSE_LEVEL:-1}"
MURMUR_MESSAGE_LOG_REGEX="${MURMUR_MESSAGE_LOG_REGEX:-TextMessage|text message|message from|sent message}"

# Generate self-signed certificate if not exists
if [ ! -f /data/cert.pem ]; then
    echo "Generating self-signed SSL certificate..."
    openssl req -x509 -newkey rsa:2048 -keyout /data/key.pem -out /data/cert.pem \
        -days 365 -nodes -subj "/CN=${MURMUR_SERVER_NAME:-Harmony}"
fi

# Substitute environment variables in config template
envsubst < /config/murmur.ini.template > /data/murmur.ini

# Find murmurd binary (location varies by distro)
MURMURD=$(which murmurd || which mumble-server || echo "/usr/bin/murmurd")

VERBOSE_FLAGS=""
if [[ "$MURMUR_VERBOSE_LEVEL" =~ ^[0-9]+$ ]] && [ "$MURMUR_VERBOSE_LEVEL" -gt 0 ]; then
    for ((i=0; i<MURMUR_VERBOSE_LEVEL; i++)); do
        VERBOSE_FLAGS="${VERBOSE_FLAGS}v"
    done
    VERBOSE_FLAGS="-${VERBOSE_FLAGS}"
fi

# Set SuperUser password if provided
if [ -n "$MURMUR_SUPERUSER_PASSWORD" ]; then
    echo "Setting SuperUser password..."
    $MURMURD -ini /data/murmur.ini -supw "$MURMUR_SUPERUSER_PASSWORD" 2>/dev/null || true
fi

echo "Starting Murmur server..."
echo "Server name: ${MURMUR_SERVER_NAME:-Harmony}"
echo "Max users: ${MURMUR_MAX_USERS:-30}"
echo "Port: 64738 (TCP)"
echo "Log file: ${MURMUR_LOG_FILE}"
echo "Verbose level: ${MURMUR_VERBOSE_LEVEL}"

TAIL_PID=""
if [ "$MURMUR_STREAM_LOGS" = "1" ] || [ "$MURMUR_STREAM_LOGS" = "true" ]; then
    touch "$MURMUR_LOG_FILE"
    tail -n0 -F "$MURMUR_LOG_FILE" | while IFS= read -r line; do
        echo "$line"
        if printf '%s\n' "$line" | grep -Eiq "$MURMUR_MESSAGE_LOG_REGEX"; then
            echo "[murmur-message] $line"
        fi
    done &
    TAIL_PID=$!
fi

# Start Murmur in foreground with configurable verbosity
MURMUR_CMD=("$MURMURD" -ini /data/murmur.ini -fg)
if [ -n "$VERBOSE_FLAGS" ]; then
    MURMUR_CMD+=("$VERBOSE_FLAGS")
fi

"${MURMUR_CMD[@]}" &
MURMUR_PID=$!
set +e
wait $MURMUR_PID
EXIT_CODE=$?
set -e

if [ -n "$TAIL_PID" ]; then
    kill "$TAIL_PID" 2>/dev/null || true
fi

exit $EXIT_CODE
