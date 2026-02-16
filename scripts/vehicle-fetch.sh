#!/bin/bash
# Vehicle data fetch script with failure notification
# Called by cron every 10 minutes on GCE

LOG_FILE="/opt/browser-render/logs/vehicle-cron.log"
EMAIL="m.tama.ramu@gmail.com"
HOSTNAME=$(hostname)

send_alert() {
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

echo "[$(date '+%Y-%m-%d %H:%M:%S')] Starting vehicle data fetch..." >> "$LOG_FILE"

# Execute the API call and capture response
RESPONSE=$(curl -sf http://localhost:8080/v1/vehicle/data 2>&1)
EXIT_CODE=$?

if [ $EXIT_CODE -ne 0 ]; then
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] FAILED: curl exit code $EXIT_CODE" >> "$LOG_FILE"
    echo "Response: $RESPONSE" >> "$LOG_FILE"
    DOCKER_LOGS=$(docker logs browser-render --since 15m 2>&1 | tail -100)
    send_alert "[ALERT] Vehicle Fetch Failed - $HOSTNAME" "Vehicle data fetch failed on $HOSTNAME\n\nTime: $(date)\nExit code: $EXIT_CODE\nResponse: $RESPONSE\n\nRecent logs:\n$DOCKER_LOGS"
    exit 1
fi

# Extract job_id from response
JOB_ID=$(echo "$RESPONSE" | jq -r '.job_id // empty')
if [ -z "$JOB_ID" ]; then
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] FAILED: Could not get job_id" >> "$LOG_FILE"
    echo "Response: $RESPONSE" >> "$LOG_FILE"
    send_alert "[ALERT] Vehicle Fetch Failed - $HOSTNAME" "Vehicle data fetch failed - no job_id\n\nTime: $(date)\nResponse: $RESPONSE"
    exit 1
fi

echo "[$(date '+%Y-%m-%d %H:%M:%S')] Job created: $JOB_ID" >> "$LOG_FILE"

# Wait for job completion (max 5 minutes)
for i in {1..30}; do
    sleep 10
    STATUS=$(curl -sf "http://localhost:8080/v1/job/$JOB_ID" 2>&1)
    JOB_STATUS=$(echo "$STATUS" | jq -r '.status // empty')

    if [ "$JOB_STATUS" = "completed" ]; then
        VEHICLE_COUNT=$(echo "$STATUS" | jq -r '.vehicle_count // 0')
        echo "[$(date '+%Y-%m-%d %H:%M:%S')] SUCCESS: $VEHICLE_COUNT vehicles fetched" >> "$LOG_FILE"
        exit 0
    elif [ "$JOB_STATUS" = "failed" ]; then
        ERROR=$(echo "$STATUS" | jq -r '.error // "Unknown error"')
        echo "[$(date '+%Y-%m-%d %H:%M:%S')] FAILED: $ERROR" >> "$LOG_FILE"
        DOCKER_LOGS=$(docker logs browser-render --since 15m 2>&1 | tail -100)
        send_alert "[ALERT] Vehicle Fetch Failed - $HOSTNAME" "Vehicle data fetch job failed\n\nTime: $(date)\nJob ID: $JOB_ID\nError: $ERROR\n\nRecent logs:\n$DOCKER_LOGS"
        exit 1
    fi
done

echo "[$(date '+%Y-%m-%d %H:%M:%S')] TIMEOUT: Job did not complete in 5 minutes" >> "$LOG_FILE"
DOCKER_LOGS=$(docker logs browser-render --since 15m 2>&1 | tail -100)
send_alert "[ALERT] Vehicle Fetch Timeout - $HOSTNAME" "Vehicle data fetch job timed out\n\nTime: $(date)\nJob ID: $JOB_ID\n\nRecent logs:\n$DOCKER_LOGS"
exit 1
