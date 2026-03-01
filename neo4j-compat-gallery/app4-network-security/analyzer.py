"""
Network Security Analyzer
Cypher-based security analysis queries for the enterprise network graph.
"""


def find_attack_paths(session, source='Internet', target=None):
    """
    Find attack paths from an external source to critical internal assets.
    If target is None, finds paths to all critical hosts.
    Returns list of paths with risk annotations.
    """
    if target:
        result = session.run("""
            MATCH path = (src {name: $source})-[:CAN_REACH*1..6]->(tgt:Host {name: $target})
            WITH path, length(path) AS hops,
                 [n IN nodes(path) WHERE n:Host | n.name] AS host_chain
            RETURN host_chain, hops
            ORDER BY hops
            LIMIT 10
        """, source=source, target=target)
    else:
        result = session.run("""
            MATCH path = (src {name: $source})-[:CAN_REACH*1..6]->(tgt:Host {critical: true})
            WITH path, tgt, length(path) AS hops,
                 [n IN nodes(path) WHERE n:Host | n.name] AS host_chain
            RETURN tgt.name AS target, host_chain, hops
            ORDER BY hops
            LIMIT 20
        """, source=source)

    paths = []
    for record in result:
        path_info = {
            'target': record.get('target', target),
            'host_chain': record['host_chain'],
            'hops': record['hops'],
        }
        paths.append(path_info)
    return paths


def find_exposed_services(session):
    """
    Find services reachable from the Internet through the DMZ.
    These represent the external attack surface.
    """
    result = session.run("""
        MATCH (inet:Network {name: 'Internet'})-[:CAN_REACH]->(h:Host)-[:IN_SUBNET]->(s:Subnet {name: 'DMZ'})
        MATCH (h)-[:RUNS]->(svc:Service)
        OPTIONAL MATCH (h)-[:HAS_VULN]->(v:Vulnerability)
        RETURN h.name AS host, h.ip AS ip, h.role AS role,
               collect(DISTINCT svc.name) AS services,
               collect(DISTINCT svc.port) AS ports,
               collect(DISTINCT v.cve) AS vulnerabilities
        ORDER BY h.name
    """)

    exposed = []
    for record in result:
        exposed.append({
            'host': record['host'],
            'ip': record['ip'],
            'role': record['role'],
            'services': record['services'],
            'ports': record['ports'],
            'vulnerabilities': record['vulnerabilities'],
        })
    return exposed


def find_critical_vulns(session):
    """
    Find all hosts with critical or high severity vulnerabilities.
    Returns hosts grouped by vulnerability with risk context.
    """
    result = session.run("""
        MATCH (h:Host)-[:HAS_VULN]->(v:Vulnerability)
        MATCH (h)-[:IN_SUBNET]->(s:Subnet)
        RETURN v.cve AS cve, v.name AS vuln_name, v.severity AS severity,
               v.cvss AS cvss, v.category AS category,
               h.name AS host, h.ip AS ip, s.name AS subnet,
               h.critical AS host_critical
        ORDER BY v.cvss DESC, h.name
    """)

    vulns = []
    for record in result:
        vulns.append({
            'cve': record['cve'],
            'vuln_name': record['vuln_name'],
            'severity': record['severity'],
            'cvss': record['cvss'],
            'category': record['category'],
            'host': record['host'],
            'ip': record['ip'],
            'subnet': record['subnet'],
            'host_critical': record['host_critical'],
        })
    return vulns


def find_lateral_paths(session):
    """
    Find possible lateral movement paths between subnets.
    Lateral movement = an attacker moving from one compromised host to another.
    """
    result = session.run("""
        MATCH (h1:Host)-[:IN_SUBNET]->(s1:Subnet),
              (h2:Host)-[:IN_SUBNET]->(s2:Subnet)
        WHERE s1.name <> s2.name
        MATCH (h1)-[:CAN_REACH]->(h2)
        MATCH (h2)-[:RUNS]->(svc:Service)
        OPTIONAL MATCH (h2)-[:HAS_VULN]->(v:Vulnerability)
        RETURN s1.name AS from_subnet, h1.name AS from_host,
               s2.name AS to_subnet, h2.name AS to_host,
               collect(DISTINCT svc.name) AS via_services,
               collect(DISTINCT v.cve) AS target_vulns
        ORDER BY s1.name, s2.name
    """)

    paths = []
    for record in result:
        paths.append({
            'from_subnet': record['from_subnet'],
            'from_host': record['from_host'],
            'to_subnet': record['to_subnet'],
            'to_host': record['to_host'],
            'via_services': record['via_services'],
            'target_vulns': record['target_vulns'],
        })
    return paths


def network_risk_score(session):
    """
    Calculate overall network risk metrics.
    Returns a composite risk assessment.
    """
    # Total hosts
    total_hosts_r = session.run(
        "MATCH (h:Host) RETURN count(h) AS cnt"
    ).single()
    total_hosts = total_hosts_r['cnt']

    # Vulnerable hosts
    vuln_hosts_r = session.run(
        "MATCH (h:Host)-[:HAS_VULN]->() RETURN count(DISTINCT h) AS cnt"
    ).single()
    vuln_hosts = vuln_hosts_r['cnt']

    # Total vulnerabilities
    total_vulns_r = session.run(
        "MATCH (v:Vulnerability) RETURN count(v) AS cnt"
    ).single()
    total_vulns = total_vulns_r['cnt']

    # Critical vulnerabilities
    crit_vulns_r = session.run(
        "MATCH (v:Vulnerability {severity: 'critical'}) RETURN count(v) AS cnt"
    ).single()
    critical_vulns = crit_vulns_r['cnt']

    # Critical hosts
    crit_hosts_r = session.run(
        "MATCH (h:Host {critical: true}) RETURN count(h) AS cnt"
    ).single()
    critical_hosts = crit_hosts_r['cnt']

    # Critical hosts with vulns
    crit_hosts_vuln_r = session.run(
        "MATCH (h:Host {critical: true})-[:HAS_VULN]->() RETURN count(DISTINCT h) AS cnt"
    ).single()
    critical_hosts_with_vulns = crit_hosts_vuln_r['cnt']

    counts = {
        'total_hosts': total_hosts,
        'vuln_hosts': vuln_hosts,
        'total_vulns': total_vulns,
        'critical_vulns': critical_vulns,
        'critical_hosts': critical_hosts,
        'critical_hosts_with_vulns': critical_hosts_with_vulns,
    }

    # Exposed surface
    exposed_hosts_r = session.run(
        "MATCH (inet:Network {name: 'Internet'})-[:CAN_REACH]->(h:Host) RETURN count(DISTINCT h) AS cnt"
    ).single()
    exposed_svcs_r = session.run(
        "MATCH (inet:Network {name: 'Internet'})-[:CAN_REACH]->(h:Host)-[:RUNS]->(svc:Service) RETURN count(DISTINCT svc) AS cnt"
    ).single()
    exposed = {
        'exposed_hosts': exposed_hosts_r['cnt'],
        'exposed_services': exposed_svcs_r['cnt'],
    }

    # Average CVSS
    avg_cvss = session.run("""
        MATCH (h:Host)-[:HAS_VULN]->(v:Vulnerability)
        RETURN avg(v.cvss) AS avg_cvss, max(v.cvss) AS max_cvss
    """).single()

    # Subnet risk breakdown - separate queries per subnet to avoid inline CASE/count
    subnet_names_r = session.run("MATCH (s:Subnet) RETURN s.name AS name ORDER BY s.name")
    subnet_breakdown = []
    for rec in subnet_names_r:
        sname = rec['name']
        sub_hosts = session.run(
            "MATCH (h:Host)-[:IN_SUBNET]->(s:Subnet {name: $sname}) RETURN count(h) AS cnt",
            sname=sname
        ).single()['cnt']
        sub_vulns = session.run(
            "MATCH (h:Host)-[:IN_SUBNET]->(s:Subnet {name: $sname}) "
            "MATCH (h)-[:HAS_VULN]->(v:Vulnerability) RETURN count(DISTINCT v) AS cnt",
            sname=sname
        ).single()['cnt']
        sub_avg_r = session.run(
            "MATCH (h:Host)-[:IN_SUBNET]->(s:Subnet {name: $sname}) "
            "MATCH (h)-[:HAS_VULN]->(v:Vulnerability) RETURN avg(v.cvss) AS avg_cvss",
            sname=sname
        ).single()
        sub_avg = sub_avg_r['avg_cvss'] if sub_avg_r['avg_cvss'] is not None else 0
        subnet_breakdown.append({
            'subnet': sname,
            'hosts': sub_hosts,
            'vulns': sub_vulns,
            'avg_cvss': round(float(sub_avg), 1),
        })
    subnet_breakdown.sort(key=lambda x: x['avg_cvss'], reverse=True)

    exposed_hosts = exposed['exposed_hosts']
    critical_hosts_vuln = counts['critical_hosts_with_vulns']

    # Composite risk score (0-100)
    # Weighted factors:
    #   40% - critical vulnerabilities on critical hosts
    #   25% - overall vulnerability coverage
    #   20% - external exposure
    #   15% - average CVSS severity
    critical_factor = min(1.0, critical_hosts_vuln / max(1, critical_hosts)) * 40
    coverage_factor = (vuln_hosts / max(1, total_hosts)) * 25
    exposure_factor = (exposed_hosts / max(1, total_hosts)) * 20
    severity_factor = (float(avg_cvss['avg_cvss'] or 0) / 10.0) * 15

    risk_score = round(critical_factor + coverage_factor + exposure_factor + severity_factor, 1)

    return {
        'risk_score': risk_score,
        'risk_grade': _risk_grade(risk_score),
        'total_hosts': total_hosts,
        'vulnerable_hosts': vuln_hosts,
        'total_vulnerabilities': total_vulns,
        'critical_vulnerabilities': critical_vulns,
        'critical_hosts': critical_hosts,
        'critical_hosts_with_vulns': critical_hosts_vuln,
        'exposed_hosts': exposed_hosts,
        'exposed_services': exposed['exposed_services'],
        'avg_cvss': round(float(avg_cvss['avg_cvss'] or 0), 1),
        'max_cvss': float(avg_cvss['max_cvss'] or 0),
        'subnet_breakdown': subnet_breakdown,
    }


def get_full_graph(session):
    """
    Return the full network graph as nodes and edges for visualization.
    """
    nodes = []
    edges = []

    # Hosts
    result = session.run("""
        MATCH (h:Host)-[:IN_SUBNET]->(s:Subnet)
        OPTIONAL MATCH (h)-[:HAS_VULN]->(v:Vulnerability)
        RETURN h.name AS name, h.ip AS ip, h.os AS os, h.role AS role,
               h.critical AS critical, s.name AS subnet,
               count(v) AS vuln_count
    """)
    for r in result:
        nodes.append({
            'id': r['name'],
            'label': r['name'] + '\n' + r['ip'],
            'type': 'host',
            'group': r['subnet'],
            'role': r['role'],
            'os': r['os'],
            'critical': r['critical'],
            'vuln_count': r['vuln_count'],
        })

    # Subnets
    result = session.run("MATCH (s:Subnet) RETURN s.name AS name, s.cidr AS cidr, s.zone AS zone, s.risk_level AS risk")
    for r in result:
        nodes.append({
            'id': 'subnet_' + r['name'],
            'label': r['name'] + '\n' + r['cidr'],
            'type': 'subnet',
            'group': r['name'],
            'zone': r['zone'],
            'risk': r['risk'],
        })

    # Internet
    nodes.append({
        'id': 'Internet',
        'label': 'Internet\n(External)',
        'type': 'network',
        'group': 'external',
    })

    # Firewalls
    result = session.run("MATCH (fw:Firewall) RETURN fw.name AS name, fw.rule_id AS rule_id, fw.action AS action")
    for r in result:
        nodes.append({
            'id': r['name'],
            'label': r['name'] + '\n' + r['rule_id'],
            'type': 'firewall',
            'group': 'firewall',
            'action': r['action'],
        })

    # Host -> Subnet edges
    result = session.run("MATCH (h:Host)-[:IN_SUBNET]->(s:Subnet) RETURN h.name AS host, s.name AS subnet")
    for r in result:
        edges.append({
            'from': r['host'],
            'to': 'subnet_' + r['subnet'],
            'type': 'in_subnet',
            'label': 'IN_SUBNET',
        })

    # CAN_REACH edges (cross-subnet only to avoid clutter)
    result = session.run("""
        MATCH (a)-[r:CAN_REACH]->(b)
        WHERE r.type = 'external' OR (a:Host AND b:Host)
        RETURN
            CASE WHEN a:Network THEN a.name ELSE a.name END AS src,
            b.name AS tgt,
            r.type AS reach_type
    """)
    for r in result:
        edges.append({
            'from': r['src'],
            'to': r['tgt'],
            'type': 'can_reach',
            'reach_type': r['reach_type'],
        })

    # Firewall edges
    result = session.run("""
        MATCH (src)-[:FILTERED_BY]->(fw:Firewall)-[a:ALLOWS]->(tgt)
        RETURN CASE WHEN src:Network THEN src.name ELSE 'subnet_' + src.name END AS src,
               fw.name AS fw,
               CASE WHEN tgt:Network THEN tgt.name ELSE 'subnet_' + tgt.name END AS tgt,
               a.service AS service
    """)
    for r in result:
        edges.append({
            'from': r['src'],
            'to': r['fw'],
            'type': 'filtered_by',
        })
        edges.append({
            'from': r['fw'],
            'to': r['tgt'],
            'type': 'allows',
            'label': r['service'],
        })

    # Deny edges
    result = session.run("""
        MATCH (src)-[:FILTERED_BY]->(fw:Firewall)-[:DENIES]->(tgt:Subnet)
        RETURN CASE WHEN src:Network THEN src.name ELSE 'subnet_' + src.name END AS src,
               fw.name AS fw,
               'subnet_' + tgt.name AS tgt
    """)
    for r in result:
        edges.append({
            'from': r['fw'],
            'to': r['tgt'],
            'type': 'denies',
            'label': 'DENY',
        })

    return {'nodes': nodes, 'edges': edges}


def _risk_grade(score):
    """Convert numeric risk score to letter grade."""
    if score >= 70:
        return 'F'
    elif score >= 55:
        return 'D'
    elif score >= 40:
        return 'C'
    elif score >= 25:
        return 'B'
    else:
        return 'A'
