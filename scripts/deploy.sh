#!/bin/bash
set -e

# Configuration
IMAGE="ghcr.io/yhonda-ohishi-pub-dev/browser-render-rust"
TAG=$(git rev-parse --short HEAD)
GCE_INSTANCE="instance-20251207-115015"
GCE_ZONE="asia-northeast1-b"
GCE_PROJECT="cloudsql-sv"
CONTAINER_NAME="browser-render"

echo "=== Browser Render Rust - Deploy Script ==="
echo "Image: ${IMAGE}:${TAG}"
echo ""

# Build Docker image
echo "=== Building Docker image ==="
docker build -t ${IMAGE}:${TAG} -t ${IMAGE}:latest .

# Push to GHCR
echo ""
echo "=== Pushing to GHCR ==="
docker push ${IMAGE}:${TAG}
docker push ${IMAGE}:latest

# Deploy to GCE
echo ""
echo "=== Deploying to GCE ==="
gcloud compute ssh ${GCE_INSTANCE} \
    --zone=${GCE_ZONE} \
    --project=${GCE_PROJECT} \
    --command="
set -e

# Login to GHCR (uses gcloud auth token)
echo 'Pulling new image...'
docker pull ${IMAGE}:${TAG}

# Stop and remove existing container
echo 'Stopping existing container...'
docker stop ${CONTAINER_NAME} 2>/dev/null || true
docker rm ${CONTAINER_NAME} 2>/dev/null || true

# Start new container
echo 'Starting new container...'
docker run -d \
    --name ${CONTAINER_NAME} \
    --restart=unless-stopped \
    -p 8080:8080 \
    -p 50051:50051 \
    -v /opt/browser-render/data:/app/data \
    -v /opt/browser-render/logs:/app/logs \
    --env-file /opt/browser-render/.env \
    --shm-size=2g \
    --security-opt seccomp=unconfined \
    ${IMAGE}:${TAG}

# Health check
echo 'Waiting for health check...'
for i in {1..15}; do
    if curl -sf http://localhost:8080/health > /dev/null 2>&1; then
        echo 'Health check passed!'
        docker ps -f name=${CONTAINER_NAME}
        exit 0
    fi
    echo \"Waiting... (\$i/15)\"
    sleep 2
done

echo 'Health check failed!'
docker logs ${CONTAINER_NAME}
exit 1
"

echo ""
echo "=== Deploy complete ==="
echo "Image: ${IMAGE}:${TAG}"
