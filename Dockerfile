# --- stage 1: build frontend assets ---
FROM node:20-alpine AS frontend

WORKDIR /fe

# Install deps first for better caching
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci

# Build the bundle
COPY frontend/ ./
ENV DIST_DIR=/fe/dist
RUN npm run build

# --- stage 2: Python runtime ---
FROM python:3.13-slim

WORKDIR /app

# Install uv
COPY --from=ghcr.io/astral-sh/uv:latest /uv /uvx /bin/

# Copy dependency files first for caching
COPY pyproject.toml uv.lock ./

# Install dependencies
RUN uv sync --frozen --no-dev

# Copy application code
COPY . .

# Copy bundled frontend assets from stage 1
COPY --from=frontend /fe/dist/ ./themes/build/static/dist/

# Add venv to PATH so skrift setup can find the skrift CLI
ENV PATH="/app/.venv/bin:$PATH"

# Expose port
EXPOSE 8080

# Run the application
CMD ["skrift", "serve", "--host", "0.0.0.0", "--port", "8080"]
