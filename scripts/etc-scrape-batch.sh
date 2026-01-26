#!/bin/bash
# ETC scrape batch script with failure notification

LOG_FILE="/opt/browser-render/logs/etc-cron.log"
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

echo "[$(date '+%Y-%m-%d %H:%M:%S')] Starting ETC batch scrape..." >> "$LOG_FILE"

# Execute the API call and capture response
RESPONSE=$(curl -sf -X POST http://localhost:8080/v1/etc/scrape/batch/env/queue 2>&1)
EXIT_CODE=$?

if [ $EXIT_CODE -ne 0 ]; then
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] FAILED: curl exit code $EXIT_CODE" >> "$LOG_FILE"
    echo "Response: $RESPONSE" >> "$LOG_FILE"
    send_alert "[ALERT] ETC Batch Scrape Failed - $HOSTNAME" "ETC batch scrape failed on $HOSTNAME\n\nTime: $(date)\nExit code: $EXIT_CODE\nResponse: $RESPONSE\n\nCheck logs: docker logs browser-render --since 10m"
    exit 1
fi

# Extract job_id from response
JOB_ID=$(echo "$RESPONSE" | jq -r '.job_id // empty')
if [ -z "$JOB_ID" ]; then
    echo "[$(date '+%Y-%m-%d %H:%M:%S')] FAILED: Could not get job_id" >> "$LOG_FILE"
    echo "Response: $RESPONSE" >> "$LOG_FILE"
    send_alert "[ALERT] ETC Batch Scrape Failed - $HOSTNAME" "ETC batch scrape failed - no job_id\n\nTime: $(date)\nResponse: $RESPONSE"
    exit 1
fi

echo "[$(date '+%Y-%m-%d %H:%M:%S')] Job queued: $JOB_ID" >> "$LOG_FILE"

# Wait for job completion (max 10 minutes - batch processing takes longer)
for i in {1..60}; do
    sleep 10
    STATUS=$(curl -sf "http://localhost:8080/v1/job/$JOB_ID" 2>&1)
    JOB_STATUS=$(echo "$STATUS" | jq -r '.status // empty')

    if [ "$JOB_STATUS" = "completed" ]; then
        ACCOUNTS=$(echo "$STATUS" | jq -r '.batch_accounts // []')
        echo "[$(date '+%Y-%m-%d %H:%M:%S')] SUCCESS: Job completed" >> "$LOG_FILE"
        echo "Accounts: $ACCOUNTS" >> "$LOG_FILE"
        exit 0
    elif [ "$JOB_STATUS" = "failed" ]; then
        ERROR=$(echo "$STATUS" | jq -r '.error // "Unknown error"')
        ACCOUNTS=$(echo "$STATUS" | jq -r '.batch_accounts // []')
        echo "[$(date '+%Y-%m-%d %H:%M:%S')] FAILED: $ERROR" >> "$LOG_FILE"
        echo "Accounts: $ACCOUNTS" >> "$LOG_FILE"
        DOCKER_LOGS=$(docker logs browser-render --since 10m 2>&1 | tail -100)
        send_alert "[ALERT] ETC Batch Scrape Failed - $HOSTNAME" "ETC batch scrape job failed\n\nTime: $(date)\nJob ID: $JOB_ID\nError: $ERROR\nAccounts: $ACCOUNTS\n\nRecent logs:\n$DOCKER_LOGS"
        exit 1
    fi
    # Job might still be queued or running, continue waiting
done

echo "[$(date '+%Y-%m-%d %H:%M:%S')] TIMEOUT: Job did not complete in 10 minutes" >> "$LOG_FILE"
send_alert "[ALERT] ETC Batch Scrape Timeout - $HOSTNAME" "ETC batch scrape job timed out\n\nTime: $(date)\nJob ID: $JOB_ID"
exit 1
