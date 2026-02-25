#!/bin/bash
set -e

# Configuration
IMAGE="ghcr.io/yhonda-ohishi-pub-dev/browser-render-rust"
TAG=$(git rev-parse --short HEAD)
CONTAINER_NAME="browser-render"
VPS_HOST="ubuntu@133.18.162.83"
VPS_KEY="$HOME/.ssh/kagoya.key"

echo "=== Browser Render Rust - Deploy to Kagoya VPS ==="
echo "Image: ${IMAGE}:${TAG}"
echo ""

# Build Docker image (limit parallelism to prevent system overload)
echo "=== Building Docker image ==="
docker build \
    --build-arg CARGO_BUILD_JOBS=2 \
    --cpuset-cpus="0-3" \
    -t ${IMAGE}:${TAG} -t ${IMAGE}:latest .

# Push to GHCR
echo ""
echo "=== Pushing to GHCR ==="
docker push ${IMAGE}:${TAG}
docker push ${IMAGE}:latest

# Deploy to Kagoya VPS
echo ""
echo "=== Deploying to Kagoya VPS ==="
ssh -i ${VPS_KEY} ${VPS_HOST} "
set -e

# Login to GHCR using token from .env
if [ -f /opt/browser-render/.env ]; then
    GHCR_TOKEN=\$(grep GHCR_TOKEN /opt/browser-render/.env | cut -d= -f2)
    if [ -n \"\$GHCR_TOKEN\" ]; then
        echo \"\$GHCR_TOKEN\" | docker login ghcr.io -u yhonda-ohishi-pub-dev --password-stdin
    fi
fi

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
    --init \
    -p 127.0.0.1:8080:8080 \
    -p 127.0.0.1:50051:50051 \
    -v /opt/browser-render/data:/app/data \
    -v /opt/browser-render/logs:/app/logs \
    -v /opt/browser-render/downloads:/app/downloads \
    --env-file /opt/browser-render/.env \
    --shm-size=1g \
    --ulimit nofile=65536:65536 \
    --security-opt seccomp=unconfined \
    ${IMAGE}:${TAG}

# Health check
echo 'Waiting for health check...'
for i in {1..15}; do
    if curl -sf http://localhost:8080/health > /dev/null 2>&1; then
        echo 'Health check passed!'
        docker ps -f name=${CONTAINER_NAME}
        # Clean up old images
        echo 'Cleaning up old images...'
        docker image prune -af --filter 'until=24h'
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
