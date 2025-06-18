# Build Platform Tests

This directory contains integration and end-to-end tests for the Build platform.

## Test Structure

- `integration/` - Cross-service integration tests
- `e2e/` - End-to-end user workflow tests
- `performance/` - Load and performance tests
- `security/` - Security and penetration tests
- `fixtures/` - Test data and fixtures
- `helpers/` - Test utilities and helper functions

## Running Tests

```bash
# Run all integration tests
pytest tests/integration/

# Run E2E tests
pytest tests/e2e/

# Run performance tests
pytest tests/performance/

# Run security tests
pytest tests/security/
```

## Test Categories

- **Unit Tests**: Located within each service directory
- **Integration Tests**: Test service-to-service communication
- **E2E Tests**: Test complete user workflows
- **Performance Tests**: Load testing and benchmarks
- **Security Tests**: Penetration and vulnerability tests

## Test Environment

Tests run against dedicated test services and databases. See the test configuration in `conftest.py` for environment setup.