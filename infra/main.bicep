// SedanDelivery Infrastructure
// Provisions: ACR + Container Apps Environment + Container App (Valkey + GQL module)
targetScope = 'resourceGroup'

@description('Environment name (e.g. main, pr-42, feat-myfeature)')
param environmentName string

@description('Location for all resources')
param location string = resourceGroup().location

@description('Container image tag')
param imageTag string = 'latest'

// ── Variables ─────────────────────────────────────────────────────────────────
var prefix = 'sd2'
// ACR names: 5-50 chars, alphanumeric only.
// Pad with 'x' chars to guarantee minimum 5 chars even for short env names.
var acrBase = '${prefix}${replace(environmentName, '-', '')}acr'
var acrName = take(length(acrBase) < 5 ? '${acrBase}xxxxx' : acrBase, 50)
var caeName = '${prefix}-${environmentName}-cae'
var caName = '${prefix}-${environmentName}-redis'
var logAnalyticsName = '${prefix}-${environmentName}-logs'

// ── Log Analytics ──────────────────────────────────────────────────────────────
resource logAnalytics 'Microsoft.OperationalInsights/workspaces@2022-10-01' = {
  name: logAnalyticsName
  location: location
  properties: {
    sku: {
      name: 'PerGB2018'
    }
    retentionInDays: 30
  }
}

// ── Container Registry ──────────────────────────────────────────────────────────
resource acr 'Microsoft.ContainerRegistry/registries@2023-07-01' = {
  name: acrName
  location: location
  sku: {
    name: 'Basic'
  }
  properties: {
    adminUserEnabled: true
  }
}

// ── Container Apps Environment ─────────────────────────────────────────────────
resource cae 'Microsoft.App/managedEnvironments@2024-03-01' = {
  name: caeName
  location: location
  properties: {
    appLogsConfiguration: {
      destination: 'log-analytics'
      logAnalyticsConfiguration: {
        customerId: logAnalytics.properties.customerId
        sharedKey: logAnalytics.listKeys().primarySharedKey
      }
    }
  }
}

// ── Container App: Valkey + GQL Module ────────────────────────────────────────
resource sedanRedis 'Microsoft.App/containerApps@2024-03-01' = {
  name: caName
  location: location
  properties: {
    managedEnvironmentId: cae.id
    configuration: {
      activeRevisionsMode: 'Single'
      registries: [
        {
          server: acr.properties.loginServer
          username: acr.listCredentials().username
          passwordSecretRef: 'acr-password'
        }
      ]
      secrets: [
        {
          name: 'acr-password'
          value: acr.listCredentials().passwords[0].value
        }
      ]
      ingress: {
        // Internal TCP: accessible only within the Container Apps Environment.
        // External TCP requires a custom VNet (too heavy for PR environments).
        // Connect via the internal FQDN from other apps in the same environment.
        external: false
        targetPort: 6379
        transport: 'tcp'
        exposedPort: 6379
      }
    }
    template: {
      containers: [
        {
          name: 'sedan-redis'
          image: '${acr.properties.loginServer}/sedan-delivery:${imageTag}'
          resources: {
            cpu: json('0.5')
            memory: '1Gi'
          }
          probes: [
            {
              type: 'Liveness'
              tcpSocket: {
                port: 6379
              }
              initialDelaySeconds: 15
              periodSeconds: 30
              failureThreshold: 3
            }
            {
              type: 'Readiness'
              tcpSocket: {
                port: 6379
              }
              initialDelaySeconds: 10
              periodSeconds: 10
              failureThreshold: 3
            }
          ]
        }
      ]
      scale: {
        minReplicas: 1
        maxReplicas: 1
      }
    }
  }
}

// ── Outputs ───────────────────────────────────────────────────────────────────
output acrLoginServer string = acr.properties.loginServer
output acrName string = acr.name
output containerAppFqdn string = sedanRedis.properties.configuration.ingress.fqdn
output containerAppName string = sedanRedis.name
output environmentName string = cae.name
