// SedanDelivery Infrastructure
// Provisions: VNet + NSG + Storage Account + ACR + Container Apps Environment
// The Container App is deployed separately after the image is built.
// A custom VNet is required for external TCP ingress (Bolt port 7687).
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
// uniqueString() ensures global uniqueness across subscriptions (Issue #69).
var acrName = take('sd2${take(replace(environmentName, '-', ''), 5)}${uniqueString(environmentName)}acr', 50)
var caeName = '${prefix}-${environmentName}-cae'
var logAnalyticsName = '${prefix}-${environmentName}-logs'
var vnetName = '${prefix}-${environmentName}-vnet'
var nsgName = '${prefix}-${environmentName}-cae-nsg'
// Storage account names: 3-24 chars, lowercase alphanumeric only.
// uniqueString() ensures global uniqueness (Issue #69).
var storageAccountName = take('sd2${take(replace(environmentName, '-', ''), 7)}${uniqueString(environmentName)}', 24)
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
    adminUserEnabled: false
  }
}

// ── AcrPull role assignment for Container App managed identity ────────────────
// Allows Container Apps to pull images via managed identity instead of admin credentials (Issue #67).
// The AcrPull built-in role ID is 7f951dda-4ed3-4680-a7ca-43fe172d538d.
@description('Principal ID of the Container App managed identity (set after first deployment)')
param containerAppPrincipalId string = ''

resource acrPullRoleAssignment 'Microsoft.Authorization/roleAssignments@2022-04-01' = if (!empty(containerAppPrincipalId)) {
  name: guid(acr.id, containerAppPrincipalId, '7f951dda-4ed3-4680-a7ca-43fe172d538d')
  scope: acr
  properties: {
    roleDefinitionId: subscriptionResourceId('Microsoft.Authorization/roleDefinitions', '7f951dda-4ed3-4680-a7ca-43fe172d538d')
    principalId: containerAppPrincipalId
    principalType: 'ServicePrincipal'
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

// ── NSG for Container Apps subnet ────────────────────────────────────────────
resource nsg 'Microsoft.Network/networkSecurityGroups@2023-04-01' = {
  name: nsgName
  location: location
  properties: {
    securityRules: [
      {
        name: 'AllowRedis'
        properties: {
          protocol: 'Tcp'
          sourcePortRange: '*'
          destinationPortRange: '6379'
          sourceAddressPrefix: '*'
          destinationAddressPrefix: '*'
          access: 'Allow'
          priority: 100
          direction: 'Inbound'
        }
      }
      {
        name: 'AllowBolt'
        properties: {
          protocol: 'Tcp'
          sourcePortRange: '*'
          destinationPortRange: '7687'
          sourceAddressPrefix: '*'
          destinationAddressPrefix: '*'
          access: 'Allow'
          priority: 110
          direction: 'Inbound'
        }
      }
    ]
  }
}

// ── VNet for Container Apps (required for external TCP ingress) ───────────────
resource vnet 'Microsoft.Network/virtualNetworks@2023-04-01' = {
  name: vnetName
  location: location
  properties: {
    addressSpace: {
      addressPrefixes: [
        '10.0.0.0/16'
      ]
    }
    subnets: [
      {
        name: 'container-apps'
        properties: {
          addressPrefix: '10.0.0.0/23'
          networkSecurityGroup: {
            id: nsg.id
          }
          delegations: [
            {
              name: 'Microsoft.App.environments'
              properties: {
                serviceName: 'Microsoft.App/environments'
              }
            }
          ]
        }
      }
    ]
  }
}

// ── Container Apps Environment ─────────────────────────────────────────────────
// Custom VNet requires at least one workload profile (Consumption is the default).
resource cae 'Microsoft.App/managedEnvironments@2024-03-01' = {
  name: caeName
  location: location
  properties: {
    vnetConfiguration: {
      internal: false
      infrastructureSubnetId: vnet.properties.subnets[0].id
    }
    workloadProfiles: [
      {
        name: 'Consumption'
        workloadProfileType: 'Consumption'
      }
    ]
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
      shareName: valkeyFileShare.name   // use resource reference, not string literal (Issue #72)
      accessMode: 'ReadWrite'
    }
  }
  dependsOn: [valkeyFileShare]
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
output vnetName string = vnet.name
output storageAccountName string = storageAccount.name
output fileShareName string = 'valkey-data'
// Azure Cache resource name only — keys are retrieved at deploy time via
// `az redis list-keys` so they never appear in ARM deployment history (Issue #58).
output azureCacheName string = persistenceBackend == 'azure-cache' ? redisCache.name : ''
