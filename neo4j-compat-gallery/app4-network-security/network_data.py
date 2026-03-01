"""
Network Security Graph Dataset
Enterprise network topology with hosts, subnets, firewalls, services, and vulnerabilities.
"""


def load_network_data(session):
    """Load the enterprise network dataset into the graph.

    Skips loading if network data already exists (idempotent).
    Does NOT clear existing data — other apps may share the graph.
    """
    # Check if network data already loaded (idempotent)
    result = session.run("MATCH (s:Subnet) RETURN count(s) AS cnt")
    if result.single()["cnt"] > 0:
        return _get_stats(session)

    # Only clear network-specific labels if reloading
    for label in ["Host", "Subnet", "Service", "Vulnerability", "Firewall", "Network"]:
        session.run(f"MATCH (n:{label}) DETACH DELETE n")

    # ── Subnets ──────────────────────────────────────────────────────
    session.run("""
        CREATE (dmz:Subnet {name: 'DMZ', cidr: '10.0.1.0/24', zone: 'external', risk_level: 'high'})
        CREATE (corp:Subnet {name: 'Corporate', cidr: '10.0.2.0/24', zone: 'internal', risk_level: 'medium'})
        CREATE (srv:Subnet {name: 'Server', cidr: '10.0.3.0/24', zone: 'internal', risk_level: 'high'})
        CREATE (mgmt:Subnet {name: 'Management', cidr: '10.0.4.0/24', zone: 'restricted', risk_level: 'critical'})
        CREATE (ot:Subnet {name: 'OT', cidr: '10.0.5.0/24', zone: 'isolated', risk_level: 'critical'})
    """)

    # ── Hosts ────────────────────────────────────────────────────────
    # DMZ hosts (4)
    session.run("""
        CREATE (:Host {name: 'web-proxy-01', ip: '10.0.1.10', os: 'Ubuntu 22.04', role: 'reverse_proxy', critical: false})
        CREATE (:Host {name: 'web-server-01', ip: '10.0.1.11', os: 'Ubuntu 22.04', role: 'web_server', critical: false})
        CREATE (:Host {name: 'mail-gw-01', ip: '10.0.1.12', os: 'CentOS 8', role: 'mail_gateway', critical: false})
        CREATE (:Host {name: 'vpn-gw-01', ip: '10.0.1.13', os: 'pfSense 2.7', role: 'vpn_gateway', critical: true})
    """)

    # Corporate hosts (5)
    session.run("""
        CREATE (:Host {name: 'ws-admin-01', ip: '10.0.2.10', os: 'Windows 11', role: 'admin_workstation', critical: true})
        CREATE (:Host {name: 'ws-dev-01', ip: '10.0.2.11', os: 'Ubuntu 22.04', role: 'dev_workstation', critical: false})
        CREATE (:Host {name: 'ws-dev-02', ip: '10.0.2.12', os: 'macOS 14', role: 'dev_workstation', critical: false})
        CREATE (:Host {name: 'printer-01', ip: '10.0.2.20', os: 'Embedded Linux', role: 'printer', critical: false})
        CREATE (:Host {name: 'wifi-ap-01', ip: '10.0.2.30', os: 'Aruba OS', role: 'access_point', critical: false})
    """)

    # Server hosts (5)
    session.run("""
        CREATE (:Host {name: 'db-primary-01', ip: '10.0.3.10', os: 'Ubuntu 22.04', role: 'database', critical: true})
        CREATE (:Host {name: 'db-replica-01', ip: '10.0.3.11', os: 'Ubuntu 22.04', role: 'database', critical: false})
        CREATE (:Host {name: 'app-server-01', ip: '10.0.3.20', os: 'Ubuntu 22.04', role: 'application_server', critical: true})
        CREATE (:Host {name: 'file-server-01', ip: '10.0.3.30', os: 'Windows Server 2022', role: 'file_server', critical: false})
        CREATE (:Host {name: 'ci-server-01', ip: '10.0.3.40', os: 'Ubuntu 22.04', role: 'ci_server', critical: false})
    """)

    # Management hosts (3)
    session.run("""
        CREATE (:Host {name: 'jump-box-01', ip: '10.0.4.10', os: 'Ubuntu 22.04', role: 'jump_box', critical: true})
        CREATE (:Host {name: 'monitor-01', ip: '10.0.4.20', os: 'Ubuntu 22.04', role: 'monitoring', critical: true})
        CREATE (:Host {name: 'backup-01', ip: '10.0.4.30', os: 'Ubuntu 22.04', role: 'backup_server', critical: true})
    """)

    # OT hosts (3)
    session.run("""
        CREATE (:Host {name: 'scada-hmi-01', ip: '10.0.5.10', os: 'Windows 10 LTSC', role: 'scada_hmi', critical: true})
        CREATE (:Host {name: 'plc-controller-01', ip: '10.0.5.20', os: 'Siemens S7', role: 'plc', critical: true})
        CREATE (:Host {name: 'historian-01', ip: '10.0.5.30', os: 'Windows Server 2019', role: 'historian', critical: true})
    """)

    # ── Services ─────────────────────────────────────────────────────
    session.run("""
        CREATE (:Service {name: 'HTTP', port: 80, protocol: 'TCP', encrypted: false})
        CREATE (:Service {name: 'HTTPS', port: 443, protocol: 'TCP', encrypted: true})
        CREATE (:Service {name: 'SSH', port: 22, protocol: 'TCP', encrypted: true})
        CREATE (:Service {name: 'RDP', port: 3389, protocol: 'TCP', encrypted: false})
        CREATE (:Service {name: 'PostgreSQL', port: 5432, protocol: 'TCP', encrypted: false})
        CREATE (:Service {name: 'MySQL', port: 3306, protocol: 'TCP', encrypted: false})
        CREATE (:Service {name: 'SMTP', port: 25, protocol: 'TCP', encrypted: false})
        CREATE (:Service {name: 'DNS', port: 53, protocol: 'UDP', encrypted: false})
        CREATE (:Service {name: 'SMB', port: 445, protocol: 'TCP', encrypted: false})
        CREATE (:Service {name: 'SNMP', port: 161, protocol: 'UDP', encrypted: false})
        CREATE (:Service {name: 'Modbus', port: 502, protocol: 'TCP', encrypted: false})
        CREATE (:Service {name: 'OPC-UA', port: 4840, protocol: 'TCP', encrypted: true})
        CREATE (:Service {name: 'LDAP', port: 389, protocol: 'TCP', encrypted: false})
        CREATE (:Service {name: 'VPN', port: 1194, protocol: 'UDP', encrypted: true})
        CREATE (:Service {name: 'Syslog', port: 514, protocol: 'UDP', encrypted: false})
    """)

    # ── Vulnerabilities ──────────────────────────────────────────────
    session.run("""
        CREATE (:Vulnerability {cve: 'CVE-2024-3094', name: 'XZ Utils Backdoor', severity: 'critical', cvss: 10.0, category: 'backdoor'})
        CREATE (:Vulnerability {cve: 'CVE-2023-44487', name: 'HTTP/2 Rapid Reset', severity: 'high', cvss: 7.5, category: 'dos'})
        CREATE (:Vulnerability {cve: 'CVE-2023-4966', name: 'Citrix Bleed', severity: 'critical', cvss: 9.4, category: 'info_disclosure'})
        CREATE (:Vulnerability {cve: 'CVE-2024-21762', name: 'FortiOS Out-of-Bound Write', severity: 'critical', cvss: 9.8, category: 'rce'})
        CREATE (:Vulnerability {cve: 'CVE-2023-36884', name: 'Office HTML RCE', severity: 'high', cvss: 8.8, category: 'rce'})
        CREATE (:Vulnerability {cve: 'CVE-2024-1709', name: 'ConnectWise ScreenConnect Auth Bypass', severity: 'critical', cvss: 10.0, category: 'auth_bypass'})
        CREATE (:Vulnerability {cve: 'CVE-2023-20198', name: 'Cisco IOS XE Web UI Priv Esc', severity: 'critical', cvss: 10.0, category: 'priv_esc'})
        CREATE (:Vulnerability {cve: 'CVE-2024-0012', name: 'PAN-OS Auth Bypass', severity: 'critical', cvss: 9.8, category: 'auth_bypass'})
        CREATE (:Vulnerability {cve: 'CVE-2023-46747', name: 'F5 BIG-IP Auth Bypass', severity: 'critical', cvss: 9.8, category: 'auth_bypass'})
        CREATE (:Vulnerability {cve: 'CVE-2024-23897', name: 'Jenkins CLI File Read', severity: 'high', cvss: 8.6, category: 'info_disclosure'})
        CREATE (:Vulnerability {cve: 'CVE-2023-38831', name: 'WinRAR Code Execution', severity: 'high', cvss: 7.8, category: 'rce'})
        CREATE (:Vulnerability {cve: 'CVE-2024-27198', name: 'TeamCity Auth Bypass', severity: 'critical', cvss: 9.8, category: 'auth_bypass'})
    """)

    # ── Firewalls / ACLs ─────────────────────────────────────────────
    session.run("""
        CREATE (:Firewall {name: 'fw-internet-dmz', rule_id: 'ACL-001', action: 'allow', direction: 'inbound'})
        CREATE (:Firewall {name: 'fw-dmz-corp', rule_id: 'ACL-002', action: 'allow', direction: 'east-west'})
        CREATE (:Firewall {name: 'fw-dmz-srv', rule_id: 'ACL-003', action: 'allow', direction: 'east-west'})
        CREATE (:Firewall {name: 'fw-corp-srv', rule_id: 'ACL-004', action: 'allow', direction: 'east-west'})
        CREATE (:Firewall {name: 'fw-srv-mgmt', rule_id: 'ACL-005', action: 'allow', direction: 'east-west'})
        CREATE (:Firewall {name: 'fw-mgmt-ot', rule_id: 'ACL-006', action: 'allow', direction: 'east-west'})
        CREATE (:Firewall {name: 'fw-corp-internet', rule_id: 'ACL-007', action: 'allow', direction: 'outbound'})
        CREATE (:Firewall {name: 'fw-dmz-deny-ot', rule_id: 'ACL-008', action: 'deny', direction: 'east-west'})
        CREATE (:Firewall {name: 'fw-corp-deny-ot', rule_id: 'ACL-009', action: 'deny', direction: 'east-west'})
        CREATE (:Firewall {name: 'fw-internet-deny-srv', rule_id: 'ACL-010', action: 'deny', direction: 'inbound'})
    """)

    # ── Internet node (attack source) ────────────────────────────────
    session.run("CREATE (:Network {name: 'Internet', type: 'external', risk_level: 'hostile'})")

    # ── Relationships: Host -[:IN_SUBNET]-> Subnet ───────────────────
    _link_hosts_to_subnets(session)

    # ── Relationships: Host -[:RUNS]-> Service ───────────────────────
    _link_hosts_to_services(session)

    # ── Relationships: Host -[:HAS_VULN]-> Vulnerability ─────────────
    _link_hosts_to_vulns(session)

    # ── Relationships: Firewall -[:CONNECTS]-> Subnet (from/to) ──────
    _link_firewalls(session)

    # ── Relationships: Network connectivity ──────────────────────────
    _link_network_paths(session)

    return get_load_summary(session)


def _link_hosts_to_subnets(session):
    """Place each host in its subnet."""
    dmz_hosts = ['web-proxy-01', 'web-server-01', 'mail-gw-01', 'vpn-gw-01']
    corp_hosts = ['ws-admin-01', 'ws-dev-01', 'ws-dev-02', 'printer-01', 'wifi-ap-01']
    srv_hosts = ['db-primary-01', 'db-replica-01', 'app-server-01', 'file-server-01', 'ci-server-01']
    mgmt_hosts = ['jump-box-01', 'monitor-01', 'backup-01']
    ot_hosts = ['scada-hmi-01', 'plc-controller-01', 'historian-01']

    for host in dmz_hosts:
        session.run(
            "MATCH (h:Host {name: $host}), (s:Subnet {name: 'DMZ'}) CREATE (h)-[:IN_SUBNET]->(s)",
            host=host
        )
    for host in corp_hosts:
        session.run(
            "MATCH (h:Host {name: $host}), (s:Subnet {name: 'Corporate'}) CREATE (h)-[:IN_SUBNET]->(s)",
            host=host
        )
    for host in srv_hosts:
        session.run(
            "MATCH (h:Host {name: $host}), (s:Subnet {name: 'Server'}) CREATE (h)-[:IN_SUBNET]->(s)",
            host=host
        )
    for host in mgmt_hosts:
        session.run(
            "MATCH (h:Host {name: $host}), (s:Subnet {name: 'Management'}) CREATE (h)-[:IN_SUBNET]->(s)",
            host=host
        )
    for host in ot_hosts:
        session.run(
            "MATCH (h:Host {name: $host}), (s:Subnet {name: 'OT'}) CREATE (h)-[:IN_SUBNET]->(s)",
            host=host
        )


def _link_hosts_to_services(session):
    """Map services running on each host."""
    host_services = {
        'web-proxy-01': ['HTTP', 'HTTPS'],
        'web-server-01': ['HTTP', 'HTTPS', 'SSH'],
        'mail-gw-01': ['SMTP', 'SSH'],
        'vpn-gw-01': ['VPN', 'SSH'],
        'ws-admin-01': ['RDP', 'SSH'],
        'ws-dev-01': ['SSH'],
        'ws-dev-02': ['SSH'],
        'printer-01': ['HTTP', 'SNMP'],
        'wifi-ap-01': ['HTTP', 'SNMP'],
        'db-primary-01': ['PostgreSQL', 'SSH'],
        'db-replica-01': ['PostgreSQL', 'SSH'],
        'app-server-01': ['HTTP', 'HTTPS', 'SSH'],
        'file-server-01': ['SMB', 'RDP'],
        'ci-server-01': ['HTTP', 'SSH'],
        'jump-box-01': ['SSH', 'RDP'],
        'monitor-01': ['HTTP', 'HTTPS', 'Syslog', 'SNMP'],
        'backup-01': ['SSH', 'SMB'],
        'scada-hmi-01': ['HTTP', 'Modbus', 'RDP'],
        'plc-controller-01': ['Modbus', 'OPC-UA'],
        'historian-01': ['MySQL', 'OPC-UA', 'SMB'],
    }
    for host, services in host_services.items():
        for svc in services:
            session.run(
                "MATCH (h:Host {name: $host}), (s:Service {name: $svc}) CREATE (h)-[:RUNS]->(s)",
                host=host, svc=svc
            )


def _link_hosts_to_vulns(session):
    """Map vulnerabilities to affected hosts."""
    host_vulns = {
        'web-server-01': ['CVE-2024-3094', 'CVE-2023-44487'],
        'mail-gw-01': ['CVE-2024-3094'],
        'vpn-gw-01': ['CVE-2024-21762', 'CVE-2024-0012'],
        'ws-admin-01': ['CVE-2023-36884', 'CVE-2023-38831'],
        'ws-dev-01': ['CVE-2024-3094'],
        'app-server-01': ['CVE-2023-44487', 'CVE-2023-4966'],
        'ci-server-01': ['CVE-2024-23897', 'CVE-2024-27198'],
        'file-server-01': ['CVE-2023-36884'],
        'scada-hmi-01': ['CVE-2023-20198', 'CVE-2024-1709'],
        'historian-01': ['CVE-2023-46747'],
        'printer-01': ['CVE-2023-20198'],
    }
    for host, vulns in host_vulns.items():
        for cve in vulns:
            session.run(
                "MATCH (h:Host {name: $host}), (v:Vulnerability {cve: $cve}) CREATE (h)-[:HAS_VULN]->(v)",
                host=host, cve=cve
            )


def _link_firewalls(session):
    """Connect firewalls between subnets with from/to semantics."""
    rules = [
        ('fw-internet-dmz', 'Internet', 'DMZ', ['HTTP', 'HTTPS', 'SMTP', 'VPN']),
        ('fw-dmz-corp', 'DMZ', 'Corporate', ['SSH', 'LDAP']),
        ('fw-dmz-srv', 'DMZ', 'Server', ['HTTP', 'PostgreSQL']),
        ('fw-corp-srv', 'Corporate', 'Server', ['PostgreSQL', 'SMB', 'HTTP', 'SSH']),
        ('fw-srv-mgmt', 'Server', 'Management', ['SSH', 'Syslog', 'SNMP']),
        ('fw-mgmt-ot', 'Management', 'OT', ['Modbus', 'OPC-UA', 'RDP']),
        ('fw-corp-internet', 'Corporate', 'Internet', ['HTTP', 'HTTPS', 'DNS']),
    ]
    for fw_name, from_net, to_net, allowed_svcs in rules:
        for svc in allowed_svcs:
            # Link firewall to source
            if from_net == 'Internet':
                session.run(
                    "MATCH (fw:Firewall {name: $fw}), (n:Network {name: $from_net}) "
                    "CREATE (n)-[:FILTERED_BY]->(fw)",
                    fw=fw_name, from_net=from_net
                )
            else:
                session.run(
                    "MATCH (fw:Firewall {name: $fw}), (s:Subnet {name: $from_net}) "
                    "CREATE (s)-[:FILTERED_BY]->(fw)",
                    fw=fw_name, from_net=from_net
                )
            # Link firewall to allowed service on destination
            if to_net == 'Internet':
                session.run(
                    "MATCH (fw:Firewall {name: $fw}), (n:Network {name: $to_net}), (svc:Service {name: $svc}) "
                    "CREATE (fw)-[:ALLOWS {service: $svc}]->(n)",
                    fw=fw_name, to_net=to_net, svc=svc
                )
            else:
                session.run(
                    "MATCH (fw:Firewall {name: $fw}), (s:Subnet {name: $to_net}), (svc:Service {name: $svc}) "
                    "CREATE (fw)-[:ALLOWS {service: $svc}]->(s)",
                    fw=fw_name, to_net=to_net, svc=svc
                )

    # Deny rules
    deny_rules = [
        ('fw-dmz-deny-ot', 'DMZ', 'OT'),
        ('fw-corp-deny-ot', 'Corporate', 'OT'),
        ('fw-internet-deny-srv', 'Internet', 'Server'),
    ]
    for fw_name, from_net, to_net in deny_rules:
        if from_net == 'Internet':
            session.run(
                "MATCH (fw:Firewall {name: $fw}), (n:Network {name: $from_net}), (s:Subnet {name: $to_net}) "
                "CREATE (n)-[:FILTERED_BY]->(fw) "
                "CREATE (fw)-[:DENIES]->(s)",
                fw=fw_name, from_net=from_net, to_net=to_net
            )
        else:
            session.run(
                "MATCH (fw:Firewall {name: $fw}), (s1:Subnet {name: $from_net}), (s2:Subnet {name: $to_net}) "
                "CREATE (s1)-[:FILTERED_BY]->(fw) "
                "CREATE (fw)-[:DENIES]->(s2)",
                fw=fw_name, from_net=from_net, to_net=to_net
            )


def _link_network_paths(session):
    """Create CAN_REACH edges between hosts in same or connected subnets."""
    # Hosts in the same subnet can reach each other
    session.run("""
        MATCH (h1:Host)-[:IN_SUBNET]->(s:Subnet)<-[:IN_SUBNET]-(h2:Host)
        WHERE h1.name < h2.name
        CREATE (h1)-[:CAN_REACH {type: 'local', subnet: s.name}]->(h2)
        CREATE (h2)-[:CAN_REACH {type: 'local', subnet: s.name}]->(h1)
    """)

    # Internet can reach DMZ hosts (through firewall)
    session.run("""
        MATCH (inet:Network {name: 'Internet'}), (h:Host)-[:IN_SUBNET]->(:Subnet {name: 'DMZ'})
        CREATE (inet)-[:CAN_REACH {type: 'external', through: 'fw-internet-dmz'}]->(h)
    """)

    # Cross-subnet reachability based on firewall allow rules
    # DMZ -> Server (web-server can reach app-server and db-primary via fw-dmz-srv)
    cross_subnet = [
        ('web-server-01', 'app-server-01', 'fw-dmz-srv'),
        ('web-server-01', 'db-primary-01', 'fw-dmz-srv'),
        # Corporate -> Server (devs reach app-server, file-server, ci)
        ('ws-dev-01', 'app-server-01', 'fw-corp-srv'),
        ('ws-dev-01', 'ci-server-01', 'fw-corp-srv'),
        ('ws-admin-01', 'db-primary-01', 'fw-corp-srv'),
        ('ws-admin-01', 'file-server-01', 'fw-corp-srv'),
        # Server -> Management (servers reach jump-box and monitor)
        ('app-server-01', 'jump-box-01', 'fw-srv-mgmt'),
        ('app-server-01', 'monitor-01', 'fw-srv-mgmt'),
        ('db-primary-01', 'monitor-01', 'fw-srv-mgmt'),
        # Management -> OT (jump-box reaches SCADA)
        ('jump-box-01', 'scada-hmi-01', 'fw-mgmt-ot'),
        ('jump-box-01', 'historian-01', 'fw-mgmt-ot'),
        ('monitor-01', 'historian-01', 'fw-mgmt-ot'),
    ]
    for src, tgt, fw in cross_subnet:
        session.run(
            "MATCH (h1:Host {name: $src}), (h2:Host {name: $tgt}) "
            "CREATE (h1)-[:CAN_REACH {type: 'cross_subnet', through: $fw}]->(h2)",
            src=src, tgt=tgt, fw=fw
        )


def _get_stats(session):
    """Internal helper — returns load summary dict."""
    return get_load_summary(session)


def get_load_summary(session):
    """Return counts of loaded entities."""
    hosts = session.run("MATCH (h:Host) RETURN count(h) AS cnt").single()['cnt']
    subnets = session.run("MATCH (s:Subnet) RETURN count(s) AS cnt").single()['cnt']
    services = session.run("MATCH (svc:Service) RETURN count(svc) AS cnt").single()['cnt']
    vulns = session.run("MATCH (v:Vulnerability) RETURN count(v) AS cnt").single()['cnt']
    firewalls = session.run("MATCH (fw:Firewall) RETURN count(fw) AS cnt").single()['cnt']
    return {
        'hosts': hosts,
        'subnets': subnets,
        'services': services,
        'vulnerabilities': vulns,
        'firewalls': firewalls,
    }
