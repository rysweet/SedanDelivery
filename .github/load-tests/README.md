# Performance Test Definitions

This directory contains performance test definitions for the CyberGym project using Azure Load Testing.

## Status

🚧 **DEFERRED** - Test definitions are placeholders until infrastructure is provisioned and baselines are established.

## Test Files

### `SedanDelivery-load-test.yaml`
Azure Load Testing configuration file defining:
- Test name and description
- Engine instances and load profile
- Environment variables
- Failure criteria and auto-stop rules

### `test-plan.jmx` (To be created)
JMeter test plan defining:
- HTTP request samplers for key endpoints
- Thread groups for load generation
- Timers for think time simulation
- Listeners for metrics collection

## Quick Start

**Prerequisites**:
1. Azure Load Testing resource created (see [Setup Guide](../../docs/PERFORMANCE_TESTING_SETUP.md))
2. Azure credentials configured in GitHub Secrets
3. Target environment accessible via HTTPS

**Run test manually**:

```bash
# Using Azure CLI
az load test start \
  --name "SedanDelivery-manual-test" \
  --resource-group "SedanDelivery-loadtest-rg" \
  --test-plan "test-plan.jmx" \
  --env TARGET_URL="https://SedanDelivery-integration.azurewebsites.net" \
  --env VIRTUAL_USERS="50"

# View results
az load test result show \
  --name "SedanDelivery-manual-test" \
  --resource-group "SedanDelivery-loadtest-rg" \
  --output table
```

**Run via gh-aw workflow**:

```bash
gh workflow run performance-testing.lock.yml \
  --field branch=integration \
  --field test_duration=5
```

## Test Endpoints

The performance tests cover these critical endpoints:

| Endpoint | Method | Purpose | Expected P95 |
|----------|--------|---------|--------------|
| `/health` | GET | Health check | < 100ms |
| `/api/dashboard` | GET | Dashboard load | < 500ms |
| `/api/auth/login` | POST | Authentication | < 400ms |
| `/api/data/save` | POST | Data operations | < 600ms |

## Test Parameters

**Default Configuration**:
- **Duration**: 5 minutes
- **Virtual Users**: 50 (ramped from 10)
- **Ramp-up Time**: 60 seconds
- **Think Time**: 2-5 seconds between requests
- **Error Threshold**: 5% maximum

**Failure Criteria**:
- Average response time > 1000ms
- Error percentage > 5%
- P95 response time > 2000ms

**Auto-Stop**:
- Stops test if error rate exceeds 20% over 60-second window

## Creating Test Definitions

### JMeter Test Plan (.jmx)

**To create a new JMeter test**:

1. **Install JMeter**:
   ```bash
   # macOS
   brew install jmeter

   # Linux
   wget https://dlcdn.apache.org//jmeter/binaries/apache-jmeter-5.5.tgz
   tar -xzf apache-jmeter-5.5.tgz
   ```

2. **Create test plan**:
   ```bash
   # Open JMeter GUI
   jmeter

   # Or use CLI mode
   jmeter -n -t template.jmx -l results.jtl
   ```

3. **Test Plan Structure**:
   - Thread Group: Define virtual users and ramp-up
   - HTTP Request Samplers: One per endpoint
   - Constant Timer: Add think time
   - Listeners: View Results Tree, Summary Report

4. **Save as** `.jmx` file in this directory

**Template Structure**:
```xml
<?xml version="1.0" encoding="UTF-8"?>
<jmeterTestPlan version="1.2">
  <hashTree>
    <TestPlan testname="CyberGym Performance Test">
      <elementProp name="TestPlan.user_defined_variables">
        <collectionProp name="Arguments.arguments">
          <elementProp name="TARGET_URL" elementType="Argument">
            <stringProp name="Argument.value">${__P(TARGET_URL,https://example.com)}</stringProp>
          </elementProp>
        </collectionProp>
      </elementProp>
    </TestPlan>
    <hashTree>
      <ThreadGroup testname="Users">
        <stringProp name="ThreadGroup.num_threads">${__P(VIRTUAL_USERS,50)}</stringProp>
        <stringProp name="ThreadGroup.ramp_time">60</stringProp>
        <stringProp name="ThreadGroup.duration">300</stringProp>
      </ThreadGroup>
      <hashTree>
        <HTTPSamplerProxy testname="GET /health">
          <stringProp name="HTTPSampler.domain">${TARGET_URL}</stringProp>
          <stringProp name="HTTPSampler.path">/health</stringProp>
          <stringProp name="HTTPSampler.method">GET</stringProp>
        </HTTPSamplerProxy>
      </hashTree>
    </hashTree>
  </hashTree>
</jmeterTestPlan>
```

### Azure Load Testing YAML

**Minimal configuration**:

```yaml
version: v0.1
testName: My Test
testPlan: test-plan.jmx
engineInstances: 1

env:
  - name: TARGET_URL
    value: https://example.com

failureCriteria:
  - avg(response_time_ms) > 1000
  - percentage(error) > 5
```

**Full configuration** (see `SedanDelivery-load-test.yaml` in this directory)

## Modifying Tests

**To add a new endpoint**:

1. Edit `test-plan.jmx` (or create if doesn't exist)
2. Add new HTTP Sampler element:
   ```xml
   <HTTPSamplerProxy testname="GET /new-endpoint">
     <stringProp name="HTTPSampler.domain">${TARGET_URL}</stringProp>
     <stringProp name="HTTPSampler.path">/new-endpoint</stringProp>
     <stringProp name="HTTPSampler.method">GET</stringProp>
   </HTTPSamplerProxy>
   ```
3. Update expected P95 baseline in workflow documentation
4. Recompile gh-aw workflow: `gh aw compile .github/aw/performance-testing.md`

**To adjust load profile**:

1. Edit `SedanDelivery-load-test.yaml`
2. Modify `loadProfile` section:
   ```yaml
   loadProfile:
     - type: ramp
       startUsers: 10
       endUsers: 100    # Increased from 50
       duration: 60
     - type: hold
       users: 100         # Increased from 50
       duration: 240
   ```
3. Update workflow if needed

**To change failure criteria**:

1. Edit `SedanDelivery-load-test.yaml`
2. Modify `failureCriteria`:
   ```yaml
   failureCriteria:
     - avg(response_time_ms) > 800   # More strict (was 1000)
     - percentage(error) > 3         # More strict (was 5)
   ```

## Test Data (Optional)

For tests requiring parameterized data (e.g., different user credentials):

**Create CSV file** (`test-config.csv`):
```csv
username,password
testuser1,pass123
testuser2,pass456
testuser3,pass789
```

**Reference in YAML**:
```yaml
configurationFiles:
  - test-config.csv
```

**Use in JMeter**:
```xml
<CSVDataSet>
  <stringProp name="filename">test-config.csv</stringProp>
  <stringProp name="variableNames">username,password</stringProp>
</CSVDataSet>
```

## Validation

**Validate JMeter test locally**:

```bash
# Test without load (single thread)
jmeter -n -t test-plan.jmx -l results.jtl -j jmeter.log

# View results
cat results.jtl

# Generate HTML report
jmeter -g results.jtl -o report/
open report/index.html
```

**Validate YAML configuration**:

```bash
# Lint YAML
yamllint SedanDelivery-load-test.yaml

# Validate against schema (if available)
# Schema: https://learn.microsoft.com/azure/load-testing/reference-test-config-yaml
```

## Best Practices

1. **Keep tests realistic**: Simulate actual user behavior with think time
2. **Start small**: Begin with low user count, gradually increase
3. **Monitor resources**: Watch CPU, memory, database connections during tests
4. **Isolate tests**: Use dedicated test environment, not production
5. **Version control**: Commit all test definition changes
6. **Document baselines**: Record expected performance metrics
7. **Regular updates**: Update tests as endpoints change

## Troubleshooting

### Test Fails to Start

**Check**:
- Target URL is accessible: `curl -I $TARGET_URL/health`
- JMeter file is valid: `jmeter -n -t test-plan.jmx -l /dev/null`
- Azure Load Testing resource exists: `az load show --name ... --resource-group ...`

### High Error Rates

**Investigate**:
- Application logs for errors
- Database connection pool saturation
- Network latency or timeouts
- Incorrect endpoint URLs in test

### Inconsistent Results

**Causes**:
- Test environment instability
- External dependencies (APIs, databases)
- Insufficient warm-up time
- Auto-scaling interference

**Solutions**:
- Increase test duration for better averages
- Add warm-up period (exclude from metrics)
- Run multiple times and average results
- Disable auto-scaling during tests

## Next Steps

1. **Create test-plan.jmx** with realistic user scenarios
2. **Validate locally** using JMeter CLI
3. **Run first test** manually to establish baseline
4. **Enable schedule** for automated testing
5. **Monitor results** and adjust thresholds as needed

## References

- [Azure Load Testing Docs](https://learn.microsoft.com/azure/load-testing/)
- [JMeter User Manual](https://jmeter.apache.org/usermanual/)
- [Performance Testing Best Practices](https://learn.microsoft.com/azure/architecture/best-practices/performance-testing)
- [CyberGym Performance Testing Setup](../../docs/PERFORMANCE_TESTING_SETUP.md)

---

**Status**: 🚧 Placeholder - Actual test definitions to be created during infrastructure setup
