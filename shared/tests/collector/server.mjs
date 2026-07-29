import { createServer } from 'http';

const captured = { spans: [], metricNames: [], serviceNames: new Set() };

function parseAttrs(attrs) {
  return (attrs || []).map(a => [a.key, a.value?.stringValue || '']).filter(([,v]) => v);
}
function parseLinks(links) {
  return (links || []).map(l => ({ trace_id: l.traceId || l.trace_id, span_id: l.spanId || l.span_id }));
}

function collectSpans(rsp) {
  let svc = '';
  for (const a of rsp.resource?.attributes || []) {
    if (a.key === 'service.name' && a.value?.stringValue) svc = a.value.stringValue;
  }
  if (svc) captured.serviceNames.add(svc);
  for (const ss of rsp.scopeSpans || []) {
    for (const span of ss.spans || []) {
      captured.spans.push({
        trace_id: span.traceId, span_id: span.spanId,
        parent_span_id: span.parentSpanId || '',
        name: span.name, kind: span.kind, service_name: svc,
        status_code: span.status?.code || 0,
        attributes: parseAttrs(span.attributes),
        links: parseLinks(span.links),
      });
    }
  }
}

function parseTracesJSON(body) {
  const data = JSON.parse(body);
  for (const rsp of data.resourceSpans || []) collectSpans(rsp);
}

function parseMetricsJSON(body) {
  const data = JSON.parse(body);
  for (const rm of data.resourceMetrics || []) {
    for (const sm of rm.scopeMetrics || []) {
      for (const m of sm.metrics || []) captured.metricNames.push(m.name);
    }
  }
}

const server = createServer((req, res) => {
  let body = [];
  req.on('data', c => body.push(c));
  req.on('end', () => {
    const buf = Buffer.concat(body);
    try {
      if (req.method === 'POST') {
        if (req.url === '/v1/traces') {
          parseTracesJSON(buf.toString());
        } else if (req.url === '/v1/metrics') {
          parseMetricsJSON(buf.toString());
        }
        res.writeHead(200);
      } else if (req.method === 'GET' && req.url === '/inspect') {
        res.writeHead(200, { 'Content-Type': 'application/json' });
        res.end(JSON.stringify({
          spans: captured.spans,
          service_names: [...captured.serviceNames],
          metric_names: captured.metricNames,
        }));
        return;
      } else if (req.method === 'POST' && req.url === '/reset') {
        captured.spans = []; captured.metricNames = []; captured.serviceNames.clear();
        res.writeHead(200);
      } else { res.writeHead(404); }
    } catch (e) { process.stderr.write(`Err: ${e.message}\n`); res.writeHead(500); }
    res.end();
  });
});

server.listen(4318, '0.0.0.0', () => process.stderr.write('ready\n'));
