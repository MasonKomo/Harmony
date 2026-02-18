# Harmony Voice Server (Murmur)

Self-hosted Mumble server for Harmony voice chat.

## Deploy to Railway

[![Deploy on Railway](https://railway.com/button.svg)](https://railway.com/template/TEMPLATE_ID)

### Manual Deployment

1. Install Railway CLI: `npm install -g @railway/cli`
2. Login: `railway login`
3. Create project: `railway init`
4. Add volume for persistence: `railway volume add`
5. Deploy: `railway up`
6. Get TCP endpoint: `railway domain --tcp`

## Environment Variables

The official mumble-server image uses `MUMBLE_CONFIG_*` prefixed environment variables.
Any murmur.ini setting can be set this way (uppercase, underscores for dots).

Common variables:
- `MUMBLE_SUPERUSER_PASSWORD` - Admin password (required for management)
- `MUMBLE_CONFIG_SERVER_PASSWORD` - Server password for joining (empty = no password)
- `MUMBLE_CONFIG_REGISTER_NAME` - Server name shown in client
- `MUMBLE_CONFIG_WELCOME_TEXT` - Welcome message
- `MUMBLE_CONFIG_USERS` - Maximum concurrent users (default: 100)
- `MUMBLE_CONFIG_BANDWIDTH` - Max bandwidth per user in bits/s (default: 558000)
- `MURMUR_LOG_FILE` - Murmur log file path (default: `/data/murmur.log`)
- `MURMUR_STREAM_LOGS` - Stream Murmur logs to Railway stdout (`1`/`true` to enable, default: `1`)
- `MURMUR_VERBOSE_LEVEL` - Murmur verbosity level (`0`-`3`, default: `1`)
- `MURMUR_MESSAGE_LOG_REGEX` - Case-insensitive regex used to tag message-send log lines

## Message Logging on Railway

Text chat/message events are emitted by Murmur logs. This setup now:
- enables verbose Murmur logging by default (`MURMUR_VERBOSE_LEVEL=1`)
- tails the Murmur log file into stdout so events appear in Railway logs
- adds `[murmur-message]` tagged lines when logs match `MURMUR_MESSAGE_LOG_REGEX`

To see messages in Railway:
1. Open your service logs in Railway
2. Send a text message from a connected client
3. Look for new Murmur log lines at that timestamp

## Connecting from Harmony

After deployment, Railway will provide a TCP endpoint like:
```
your-project.railway.app:12345
```

Configure this in your Harmony app's connection settings.

## Notes

- Railway only supports TCP (not UDP), so Mumble will use TCP fallback
- Voice quality is still good for small groups (2-30 users)
- Data is persisted via Railway volume

## Creating a Railway Template

1. Push this to a GitHub repo
2. Go to https://railway.com/templates
3. Click "Create Template"
4. Link your GitHub repo
5. Configure the template variables
6. Publish!
