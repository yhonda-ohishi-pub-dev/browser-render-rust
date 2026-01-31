#!/bin/bash
# Daily disk usage check with email alert

EMAIL="m.tama.ramu@gmail.com"
HOSTNAME=$(hostname)
THRESHOLD=80

send_email() {
    local subject="$1"
    local body="$2"
    {
        echo "To: $EMAIL"
        echo "From: $EMAIL"
        echo "Subject: $subject"
        echo ""
        echo -e "$body"
    } | /usr/bin/msmtp "$EMAIL"
}

# Disk usage
DISK_USAGE=$(df -h / | awk 'NR==2 {print $5}' | tr -d '%')
DISK_INFO=$(df -h /)

# Docker container /tmp size
CONTAINER_ID=$(docker ps -q 2>/dev/null)
TMP_SIZE="N/A"
TMP_COUNT="0"
if [ -n "$CONTAINER_ID" ]; then
    TMP_SIZE=$(docker exec "$CONTAINER_ID" du -sh /tmp 2>/dev/null | awk '{print $1}')
    TMP_COUNT=$(docker exec "$CONTAINER_ID" sh -c 'ls /tmp/dtakolog-* 2>/dev/null | wc -l')
fi

# Docker system usage
DOCKER_USAGE=$(docker system df 2>/dev/null)

BODY="Disk Usage Report - $(date '+%Y-%m-%d %H:%M:%S %Z')\n\n"
BODY+="=== Disk ===\n$DISK_INFO\n\n"
BODY+="=== Container /tmp ===\nSize: $TMP_SIZE\ndtakolog dirs: $TMP_COUNT\n\n"
BODY+="=== Docker ===\n$DOCKER_USAGE\n"

if [ "$DISK_USAGE" -ge "$THRESHOLD" ]; then
    send_email "[ALERT] Disk Usage ${DISK_USAGE}% - $HOSTNAME" "$BODY"
else
    send_email "[INFO] Disk Usage ${DISK_USAGE}% - $HOSTNAME" "$BODY"
fi
