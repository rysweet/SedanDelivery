// SedanDelivery Infrastructure
// Provisions: Storage Account + ACR + Container Apps Environment
// The Container App is deployed separately after the image is built.
targetScope = 'resourceGroup'

@description('Environment name (e.g. main, pr-42, feat-myfeature)')
param environmentName string

@description('Location for all resources')
param location string = resourceGroup().location

@description('Persistence backend: valkey-volume (Azure Files mount) or azure-cache (Azure Cache for Redis)')
@allowed(['valkey-volume', 'azure-cache'])
param persistenceBackend string = 'valkey-volume'

// ── Variables ─────────────────────────────────────────────────────────────────
var prefix = 'sd2'
// ACR names: 5-50 chars, alphanumeric only.
// Pad to guarantee minimum 5 chars even for short env names.
var acrBase = '${prefix}${replace(environmentName, '-', '')}acr'
var acrName = take(length(acrBase) < 5 ? '${acrBase}xxxxx' : acrBase, 50)
var caeName = '${prefix}-${environmentName}-cae'
var logAnalyticsName = '${prefix}-${environmentName}-logs'
// Storage account names: 3-24 chars, lowercase alphanumeric only.
var storageAccountName = take('sd2${replace(environmentName, '-', '')}st', 24)
// Azure Cache for Redis name: max 63 chars
var redisCacheName = take('sd2-${environmentName}-cache', 63)

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

// ── Storage Account (always provisioned) ──────────────────────────────────────
resource storageAccount 'Microsoft.Storage/storageAccounts@2023-01-01' = {
  name: storageAccountName
  location: location
  kind: 'StorageV2'
  sku: {
    name: 'Standard_LRS'
  }
  properties: {
    allowBlobPublicAccess: false
    minimumTlsVersion: 'TLS1_2'
  }
}

// ── File Share for Valkey persistence (always provisioned) ────────────────────
resource fileService 'Microsoft.Storage/storageAccounts/fileServices@2023-01-01' = {
  name: 'default'
  parent: storageAccount
}

resource valkeyFileShare 'Microsoft.Storage/storageAccounts/fileServices/shares@2023-01-01' = {
  name: 'valkey-data'
  parent: fileService
  properties: {
    shareQuota: 10
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

// ── Attach Azure File Share to CAE (always provisioned) ───────────────────────
resource caeStorage 'Microsoft.App/managedEnvironments/storages@2024-03-01' = {
  name: 'valkey-data'
  parent: cae
  properties: {
    azureFile: {
      accountName: storageAccount.name
      accountKey: storageAccount.listKeys().keys[0].value
      shareName: 'valkey-data'
      accessMode: 'ReadWrite'
    }
  }
}

// ── Azure Cache for Redis (only when persistenceBackend == 'azure-cache') ──────
resource redisCache 'Microsoft.Cache/redis@2023-08-01' = if (persistenceBackend == 'azure-cache') {
  name: redisCacheName
  location: location
  properties: {
    sku: {
      name: 'Standard'
      family: 'C'
      capacity: 1
    }
    enableNonSslPort: false
    minimumTlsVersion: '1.2'
  }
}

// ── Outputs ───────────────────────────────────────────────────────────────────
output acrLoginServer string = acr.properties.loginServer
output acrName string = acr.name
output caeName string = cae.name
output caeId string = cae.id
output storageAccountName string = storageAccount.name
output fileShareName string = 'valkey-data'
output azureCacheConnectionString string = persistenceBackend == 'azure-cache' ? '${redisCache.properties.hostName}:${redisCache.properties.sslPort},password=${redisCache.listKeys().primaryKey},ssl=True,abortConnect=False' : ''
