// SedanDelivery Infrastructure
// Provisions: ACR + Container Apps Environment
// The Container App is deployed separately after the image is built.
targetScope = 'resourceGroup'

@description('Environment name (e.g. main, pr-42, feat-myfeature)')
param environmentName string

@description('Location for all resources')
param location string = resourceGroup().location

// ── Variables ─────────────────────────────────────────────────────────────────
var prefix = 'sd2'
// ACR names: 5-50 chars, alphanumeric only.
// Pad to guarantee minimum 5 chars even for short env names.
var acrBase = '${prefix}${replace(environmentName, '-', '')}acr'
var acrName = take(length(acrBase) < 5 ? '${acrBase}xxxxx' : acrBase, 50)
var caeName = '${prefix}-${environmentName}-cae'
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

// ── Outputs ───────────────────────────────────────────────────────────────────
output acrLoginServer string = acr.properties.loginServer
output acrName string = acr.name
output caeName string = cae.name
output caeId string = cae.id
