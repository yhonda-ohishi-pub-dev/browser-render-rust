#!/bin/bash
set -e

echo "=== Browser Render Rust - GCE Setup Script ==="
echo ""

# Create application directories
echo "Creating application directories..."
sudo mkdir -p /opt/browser-render/{data,logs}
sudo chown -R $USER:$USER /opt/browser-render

# Create environment file template
echo "Creating environment file template..."
cat > /opt/browser-render/.env << 'EOF'
# Server settings
HTTP_PORT=8080
GRPC_PORT=50051

# Auth credentials (REQUIRED - update these values)
USER_NAME=your_username
COMP_ID=your_company_id
USER_PASS=your_password

# Browser settings
BROWSER_HEADLESS=true
BROWSER_TIMEOUT=60s
BROWSER_DEBUG=false

# Database
SQLITE_PATH=/app/data/browser_render.db

# Session settings
SESSION_TTL=10m
COOKIE_TTL=24h

# API settings
HONO_API_URL=https://hono-api.mtamaramu.com/api/dtakologs

# Logging
LOG_FORMAT=json
LOG_FILE=app.log
LOG_DIR=/app/logs
LOG_ROTATION=daily
EOF

echo "Created /opt/browser-render/.env"

# Install Docker if not present
if ! command -v docker &> /dev/null; then
    echo ""
    echo "Installing Docker..."
    curl -fsSL https://get.docker.com -o get-docker.sh
    sudo sh get-docker.sh
    sudo usermod -aG docker $USER
    rm get-docker.sh
    echo "Docker installed. Please log out and back in for group changes to take effect."
else
    echo "Docker is already installed."
fi

# Ensure Docker is enabled and running
sudo systemctl enable docker
sudo systemctl start docker

# Setup GHCR authentication from .env if token exists
setup_ghcr_auth() {
    if [ -f /opt/browser-render/.env ]; then
        GHCR_TOKEN=$(grep GHCR_TOKEN /opt/browser-render/.env | cut -d= -f2)
        if [ -n "$GHCR_TOKEN" ]; then
            echo "Setting up GHCR authentication..."
            echo "$GHCR_TOKEN" | sudo docker login ghcr.io -u yhonda-ohishi-pub-dev --password-stdin
            echo "GHCR authentication configured."
        else
            echo ""
            echo "Please login to GitHub Container Registry:"
            echo "Run: docker login ghcr.io -u <your-github-username>"
            echo ""
        fi
    fi
}

# Try to setup GHCR auth (will work after .env is copied with GHCR_TOKEN)
setup_ghcr_auth

# Create cleanup cron job (remove old images weekly)
(crontab -l 2>/dev/null | grep -v "docker image prune"; echo "0 3 * * 0 docker image prune -af --filter 'until=168h'") | crontab -
echo "Added weekly Docker image cleanup cron job."

echo ""
echo "=== Setup Complete ==="
echo ""
echo "Next steps:"
echo "1. Edit /opt/browser-render/.env with your credentials"
echo "2. Run: docker login ghcr.io -u yhonda-ohishi-pub-dev"
echo "3. Log out and back in if Docker was just installed"
echo ""
