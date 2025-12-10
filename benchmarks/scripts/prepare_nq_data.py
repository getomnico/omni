#!/usr/bin/env python3
"""
Prepare Natural Questions dataset for benchmarking.

This script extracts unique Wikipedia articles from the raw Natural Questions
JSONL.gz files and converts them to a format suitable for the benchmark indexer.

Output:
  - corpus.jsonl: One document per line with {id, title, text}
  - queries.jsonl: One query per line with {id, text}
  - metadata.json: Statistics about the extracted data
"""

import argparse
import gzip
import hashlib
import json
import os
import re
import sys
from collections import defaultdict
from html.parser import HTMLParser
from pathlib import Path
from typing import Dict, Generator, Optional, Set, Tuple


class HTMLTextExtractor(HTMLParser):
    """Extract plain text from HTML, handling Wikipedia article structure."""

    def __init__(self):
        super().__init__()
        self.text_parts = []
        self.skip_tags = {'script', 'style', 'sup', 'sub'}
        self.current_skip = 0

    def handle_starttag(self, tag, attrs):
        if tag in self.skip_tags:
            self.current_skip += 1
        elif tag in ('p', 'div', 'br', 'li', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6'):
            self.text_parts.append('\n')

    def handle_endtag(self, tag):
        if tag in self.skip_tags:
            self.current_skip = max(0, self.current_skip - 1)
        elif tag in ('p', 'div', 'li'):
            self.text_parts.append('\n')

    def handle_data(self, data):
        if self.current_skip == 0:
            self.text_parts.append(data)

    def get_text(self) -> str:
        text = ''.join(self.text_parts)
        # Clean up whitespace
        text = re.sub(r'\n\s*\n', '\n\n', text)
        text = re.sub(r'[ \t]+', ' ', text)
        return text.strip()


def html_to_text(html: str) -> str:
    """Convert HTML to plain text."""
    parser = HTMLTextExtractor()
    try:
        parser.feed(html)
        return parser.get_text()
    except Exception:
        # Fallback: strip tags with regex
        text = re.sub(r'<[^>]+>', ' ', html)
        text = re.sub(r'\s+', ' ', text)
        return text.strip()


def generate_doc_id(url: str) -> str:
    """Generate a deterministic document ID from URL."""
    return hashlib.md5(url.encode()).hexdigest()[:16]


def stream_nq_records(input_dir: Path) -> Generator[dict, None, None]:
    """Stream records from NQ JSONL.gz files."""
    # Find all JSONL.gz files
    patterns = ['train/nq-train-*.jsonl.gz', 'dev/nq-dev-*.jsonl.gz']

    for pattern in patterns:
        files = sorted(input_dir.glob(pattern))
        for filepath in files:
            print(f"Processing: {filepath.name}", file=sys.stderr, flush=True)
            try:
                with gzip.open(filepath, 'rt', encoding='utf-8') as f:
                    for line in f:
                        try:
                            record = json.loads(line)
                            yield record
                        except json.JSONDecodeError as e:
                            print(f"  JSON decode error: {e}", file=sys.stderr)
                            continue
            except Exception as e:
                print(f"  Error reading file: {e}", file=sys.stderr)
                continue


def extract_documents_and_queries(
    input_dir: Path,
    max_documents: Optional[int] = None,
    max_queries: Optional[int] = None,
) -> Tuple[Dict[str, dict], list]:
    """
    Extract unique documents and queries from NQ dataset.

    Returns:
        Tuple of (documents dict keyed by URL, queries list)
    """
    documents: Dict[str, dict] = {}  # url -> document
    queries = []
    seen_questions: Set[str] = set()

    for record in stream_nq_records(input_dir):
        # Stop early if we have enough
        if max_documents and len(documents) >= max_documents:
            if max_queries and len(queries) >= max_queries:
                break
            elif not max_queries and len(queries) >= max_documents * 2:
                break

        # Extract document
        doc_url = record.get('document_url', '')
        if not doc_url:
            continue

        # Only process if we haven't seen this document or need more
        if doc_url not in documents:
            if max_documents and len(documents) >= max_documents:
                pass
            else:
                # Extract document content
                doc_html = record.get('document_html', '')
                doc_title = record.get('document_title', '')

                if doc_html:
                    doc_text = html_to_text(doc_html)

                    # Skip very short documents
                    if len(doc_text) < 100:
                        continue

                    doc_id = generate_doc_id(doc_url)
                    documents[doc_url] = {
                        'id': doc_id,
                        'title': doc_title,
                        'text': doc_text,
                        'url': doc_url,
                    }

                    if len(documents) % 100 == 0:
                        print(f"  Extracted {len(documents)} documents, {len(queries)} queries", file=sys.stderr, flush=True)

        # Extract query (question)
        question = record.get('question_text', '')
        if question and question not in seen_questions:
            if max_queries and len(queries) >= max_queries:
                continue

            seen_questions.add(question)

            # Get the document ID for this question (if we have the document)
            relevant_doc_id = None
            if doc_url in documents:
                relevant_doc_id = documents[doc_url]['id']

            queries.append({
                'id': f"q{len(queries):08d}",
                'text': question,
                'relevant_doc_id': relevant_doc_id,
            })

    return documents, queries


def write_corpus(documents: Dict[str, dict], output_path: Path):
    """Write corpus to JSONL file."""
    with open(output_path, 'w', encoding='utf-8') as f:
        for doc in documents.values():
            # Write minimal format for indexing
            record = {
                'id': doc['id'],
                'title': doc['title'],
                'text': doc['text'],
            }
            f.write(json.dumps(record, ensure_ascii=False) + '\n')


def write_queries(queries: list, output_path: Path):
    """Write queries to JSONL file."""
    with open(output_path, 'w', encoding='utf-8') as f:
        for query in queries:
            f.write(json.dumps(query, ensure_ascii=False) + '\n')


def write_metadata(documents: Dict[str, dict], queries: list, output_path: Path):
    """Write metadata about the extraction."""
    total_text_bytes = sum(len(d['text'].encode('utf-8')) for d in documents.values())
    avg_doc_length = total_text_bytes / len(documents) if documents else 0

    metadata = {
        'total_documents': len(documents),
        'total_queries': len(queries),
        'total_text_bytes': total_text_bytes,
        'avg_document_length_bytes': int(avg_doc_length),
        'queries_with_relevant_docs': sum(1 for q in queries if q.get('relevant_doc_id')),
    }

    with open(output_path, 'w', encoding='utf-8') as f:
        json.dump(metadata, f, indent=2)


def main():
    parser = argparse.ArgumentParser(
        description='Prepare Natural Questions dataset for benchmarking'
    )
    parser.add_argument(
        '--input-dir',
        type=str,
        default='benchmarks/data/v1.0',
        help='Input directory containing NQ JSONL.gz files'
    )
    parser.add_argument(
        '--output-dir',
        type=str,
        default='benchmarks/data/nq_benchmark',
        help='Output directory for prepared data'
    )
    parser.add_argument(
        '--max-documents',
        type=int,
        default=None,
        help='Maximum number of documents to extract (default: all)'
    )
    parser.add_argument(
        '--max-queries',
        type=int,
        default=None,
        help='Maximum number of queries to extract (default: all)'
    )

    args = parser.parse_args()

    input_dir = Path(args.input_dir)
    output_dir = Path(args.output_dir)

    if not input_dir.exists():
        print(f"Error: Input directory does not exist: {input_dir}", file=sys.stderr)
        sys.exit(1)

    # Create output directory
    output_dir.mkdir(parents=True, exist_ok=True)

    print(f"Input directory: {input_dir}", file=sys.stderr)
    print(f"Output directory: {output_dir}", file=sys.stderr)
    if args.max_documents:
        print(f"Max documents: {args.max_documents}", file=sys.stderr)
    if args.max_queries:
        print(f"Max queries: {args.max_queries}", file=sys.stderr)
    print("", file=sys.stderr)

    # Extract documents and queries
    print("Extracting documents and queries...", file=sys.stderr)
    documents, queries = extract_documents_and_queries(
        input_dir,
        max_documents=args.max_documents,
        max_queries=args.max_queries,
    )

    print(f"\nExtracted {len(documents)} unique documents", file=sys.stderr)
    print(f"Extracted {len(queries)} unique queries", file=sys.stderr)

    # Write output files
    print("\nWriting output files...", file=sys.stderr)

    corpus_path = output_dir / 'corpus.jsonl'
    write_corpus(documents, corpus_path)
    print(f"  Written: {corpus_path}", file=sys.stderr)

    queries_path = output_dir / 'queries.jsonl'
    write_queries(queries, queries_path)
    print(f"  Written: {queries_path}", file=sys.stderr)

    metadata_path = output_dir / 'metadata.json'
    write_metadata(documents, queries, output_dir / 'metadata.json')
    print(f"  Written: {metadata_path}", file=sys.stderr)

    # Print summary
    print("\n=== Summary ===", file=sys.stderr)
    with open(metadata_path) as f:
        metadata = json.load(f)
    for key, value in metadata.items():
        print(f"  {key}: {value}", file=sys.stderr)

    print("\nDone!", file=sys.stderr)


if __name__ == '__main__':
    main()
