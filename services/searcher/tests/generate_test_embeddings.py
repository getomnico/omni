#!/usr/bin/env python3
import json
import requests
import os

# Test documents with different content for vector search testing
test_documents = [
    {
        "id": "doc1",
        "title": "Machine Learning Fundamentals",
        "content": "Machine learning is a subset of artificial intelligence that focuses on building systems that learn from data. Neural networks, deep learning, and natural language processing are key concepts in modern ML applications.",
    },
    {
        "id": "doc2",
        "title": "Database Performance Optimization",
        "content": "Database performance optimization involves tuning queries, creating proper indexes, and managing connection pools. Understanding query execution plans and database statistics is crucial for optimal performance.",
    },
    {
        "id": "doc3",
        "title": "Artificial Intelligence and Neural Networks",
        "content": "Artificial intelligence encompasses machine learning, deep learning, and neural networks. These technologies enable computers to perform tasks that typically require human intelligence, such as image recognition and natural language understanding.",
    },
    {
        "id": "doc4",
        "title": "Web Development Best Practices",
        "content": "Modern web development requires understanding of frontend frameworks, backend APIs, and database design. Security, performance, and user experience are critical considerations in building scalable web applications.",
    },
    {
        "id": "doc5",
        "title": "Deep Learning Applications",
        "content": "Deep learning has revolutionized computer vision, natural language processing, and speech recognition. Convolutional neural networks and transformers are powerful architectures used in state-of-the-art AI systems.",
    },
]

# Search queries to generate embeddings for
test_queries = [
    "machine learning neural networks",
    "database optimization performance",
    "artificial intelligence deep learning",
    "web development security",
]


def generate_embeddings(texts, task="passage"):
    """Call omni-ai service to generate embeddings"""
    response = requests.post(
        "http://localhost:3003/embeddings", json={"texts": texts, "task": task}
    )
    response.raise_for_status()
    return response.json()


def main():
    # Generate embeddings for documents
    document_texts = [f"{doc['title']} {doc['content']}" for doc in test_documents]
    doc_response = generate_embeddings(document_texts, task="passage")

    # Generate embeddings for queries
    query_response = generate_embeddings(test_queries, task="query")

    # Prepare test data
    test_data = {"documents": [], "queries": []}

    # Add document embeddings
    for i, doc in enumerate(test_documents):
        if i < len(doc_response["embeddings"]):
            test_data["documents"].append(
                {
                    "id": doc["id"],
                    "title": doc["title"],
                    "content": doc["content"],
                    "embedding": doc_response["embeddings"][i],
                    "chunks": (
                        doc_response["chunks"][i]
                        if i < len(doc_response["chunks"])
                        else []
                    ),
                    "chunks_count": (
                        doc_response["chunks_count"][i]
                        if i < len(doc_response["chunks_count"])
                        else 0
                    ),
                }
            )

    # Add query embeddings
    for i, query in enumerate(test_queries):
        if i < len(query_response["embeddings"]):
            test_data["queries"].append(
                {"text": query, "embedding": query_response["embeddings"][i]}
            )

    # Save to file
    output_dir = os.path.dirname(os.path.abspath(__file__))
    output_file = os.path.join(output_dir, "test_embeddings.json")

    with open(output_file, "w") as f:
        json.dump(test_data, f, indent=2)

    print(f"Generated embeddings saved to: {output_file}")
    print(f"Documents: {len(test_data['documents'])}")
    print(f"Queries: {len(test_data['queries'])}")


if __name__ == "__main__":
    main()
