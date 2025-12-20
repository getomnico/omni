import time
import torch
from transformers import AutoModel, AutoTokenizer
from transformers.tokenization_utils_base import BatchEncoding
import torch.nn.functional as F

model_name = "jinaai/jina-embeddings-v3"

model = AutoModel.from_pretrained(model_name, trust_remote_code=True)
model.cuda()
tokenizer = AutoTokenizer.from_pretrained(model_name, trust_remote_code=True)


def tokenize(texts: list[str]) -> BatchEncoding:
    start = time.time_ns()
    inputs = tokenizer(
        texts,
        return_tensors="pt",
        truncation=True,
        padding=True,
        return_offsets_mapping=True,
    )
    end = time.time_ns()
    print(
        f"Tokenized input: {inputs['input_ids'].shape} [took {(end - start) / 1e6:.2f} ms]"
    )
    return inputs


def forward(inputs: BatchEncoding, task="retrieval.passage") -> torch.Tensor:
    num_examples = inputs.input_ids.shape[0]
    task_id = model._adaptation_map[task]
    adapter_mask = torch.full(
        (num_examples,), task_id, dtype=torch.int32, device=model.device
    )

    inputs = {k: v.cuda() for k, v in inputs.items()}

    start = time.time_ns()
    with torch.no_grad():
        outputs = model.forward(**inputs, adapter_mask=adapter_mask, return_dict=True)
        embeddings = outputs.last_hidden_state
    end = time.time_ns()
    print(f"Embeddings: {embeddings.shape} [took {(end - start) / 1e6:.2f} ms]")
    return embeddings


def chunk_by_sentences(inputs: BatchEncoding, chunk_size: int = 512):
    start = time.time_ns()
    batch_size = inputs.input_ids.shape[0]

    eos_indices = torch.where(inputs.input_ids == tokenizer.eos_token_id)[1]

    batch_chunk_char_spans = []
    batch_chunk_token_spans = []

    for i in range(batch_size):
        tokens = inputs.tokens(i)
        offset_mapping = inputs.offset_mapping[i]

        eos_idx = int(eos_indices[i].item())

        chunk_char_spans = []
        chunk_token_spans = []
        curr_sentence_start = 1
        curr_chunk_size = 0

        for j in range(1, eos_idx):
            if tokens[j] in [".", "!", "?"] or j == eos_idx - 1:
                start_token_span = offset_mapping[curr_sentence_start]
                end_token_span = offset_mapping[j]
                sentence_len = end_token_span[1] - start_token_span[0]
                if sentence_len >= 4:
                    # Found the next sentence
                    num_tokens_in_sentence = j - curr_sentence_start + 1
                    if len(chunk_char_spans) == 0:
                        # Each chunk should have at least one sentence, regardless of chunk size
                        chunk_token_spans.append((curr_sentence_start, j + 1))
                        chunk_char_spans.append(
                            (start_token_span[0], end_token_span[1] + 1)
                        )
                        curr_sentence_start = j + 1
                        curr_chunk_size = num_tokens_in_sentence
                    elif curr_chunk_size + num_tokens_in_sentence <= chunk_size:
                        # We can include this sentence in the curr chunk
                        chunk_token_spans[-1] = (chunk_token_spans[-1][0], j + 1)
                        chunk_char_spans[-1] = (
                            chunk_char_spans[-1][0],
                            end_token_span[1] + 1,
                        )
                        curr_sentence_start = j + 1
                        curr_chunk_size += num_tokens_in_sentence
                    else:
                        # Start a new chunk
                        chunk_token_spans.append((curr_sentence_start, j + 1))
                        chunk_char_spans.append(
                            (start_token_span[0], end_token_span[1] + 1)
                        )
                        curr_sentence_start = j + 1
                        curr_chunk_size = num_tokens_in_sentence

        batch_chunk_token_spans.append(chunk_token_spans)
        batch_chunk_char_spans.append(chunk_char_spans)

    end = time.time_ns()
    print(
        f"Num chunks: {[len(x) for x in batch_chunk_char_spans]} [took {(end - start) / 1e6:.2f} ms]"
    )
    return batch_chunk_char_spans, batch_chunk_token_spans


def apply_late_chunking(
    token_embeddings: torch.Tensor, chunk_spans: list[list[tuple[int, int]]]
):
    start = time.time_ns()

    # Extract all chunks in one go
    all_chunks = [
        token_embeddings[batch_idx, chunk_start:chunk_end, :].mean(dim=0)
        for batch_idx, spans in enumerate(chunk_spans)
        for chunk_start, chunk_end in spans
    ]

    # Stack and normalize all at once
    chunks = torch.stack(all_chunks, dim=0)
    norm_chunks = F.normalize(chunks, p=2, dim=1)  # dim=1 for feature dimension

    end = time.time_ns()
    print(f"Chunked embeddings: {len(norm_chunks)} [took {(end - start) / 1e6:.2f} ms]")
    return norm_chunks


# def apply_late_chunking(token_embeddings: torch.Tensor, chunk_spans: list[list[tuple[int, int]]]):
#     start = time.time_ns()
#
#     # Process each batch separately, keep as list
#     batch_chunks = []
#
#     for batch_idx, spans in enumerate(chunk_spans):
#         chunks = [
#             F.normalize(
#                 token_embeddings[batch_idx, chunk_start:chunk_end, :].mean(dim=0),
#                 p=2,
#                 dim=0
#             )
#             for chunk_start, chunk_end in spans
#         ]
#         batch_chunks.append(torch.stack(chunks) if chunks else torch.empty(0, token_embeddings.shape[-1]))
#
#     end = time.time_ns()
#     total_chunks = sum(len(chunks) for chunks in batch_chunks)
#     print(f"Chunked embeddings: {total_chunks} across {len(batch_chunks)} batches [took {(end - start) / 1e6:.2f} ms]")
#     return batch_chunks  # List of tensors: [tensor(num_chunks_0, embed_dim), tensor(num_chunks_1, embed_dim), ...]


def print_chunks(texts: list[str], batch_chunk_char_spans: list[list[tuple[int, int]]]):
    batch_size = len(batch_chunk_char_spans)
    for i in range(batch_size):
        print(f"-" * 100)
        print(f"Text {i} has {len(batch_chunk_char_spans[i])} chunks")
        for chunk_start, chunk_end in batch_chunk_char_spans[i]:
            print(f"Chunk: {texts[i][chunk_start:chunk_end]}")


def find_k_most_similar_chunks(
    texts: list[str],
    batch_chunk_char_spans: list[list[tuple[int, int]]],
    query_embeddings: torch.Tensor,
    chunk_embeddings: torch.Tensor,
    k: int = 5,
):
    """Query embeddings should be of shape (1, d) and chunk embeddings should be of shape (n, d)"""

    similarities = torch.matmul(query_embeddings, chunk_embeddings.T)
    similarities = similarities.squeeze(0)
    top_k_indices = torch.topk(similarities, k).indices

    top_k_chunks = []
    for chunk_idx in top_k_indices:
        # Find the index in the texts array that contains this chunk idx
        batch_idx = 0
        while chunk_idx >= len(batch_chunk_char_spans[batch_idx]):
            chunk_idx -= len(batch_chunk_char_spans[batch_idx])
            batch_idx += 1

        chunk_start, chunk_end = batch_chunk_char_spans[batch_idx][chunk_idx]
        top_k_chunks.append(texts[batch_idx][chunk_start:chunk_end])
    return top_k_chunks


# batch_size = 64
texts = [
    "A capybara is a large rodent native to South America. It is the largest rodent in the world, reaching lengths of up to 4.5 feet (1.4 meters) and weighing up to 150 pounds (68 kilograms). Capybaras are semi-aquatic mammals, spending much of their time in and around bodies of water. They are known for their friendly and social nature, often living in groups called herds. Capybaras are herbivores, feeding on grasses, aquatic plants, and fruits. They are also known for their strong teeth, which they use to gnaw on wood and other tough materials.",
    "The industrial revolution was a period of significant change in the late 18th and early 19th centuries, marked by the transition from manual labor to machinery. This shift had profound effects on society, economy, and culture. The invention of the steam engine, spinning jenny, and power loom revolutionized manufacturing, leading to increased productivity and the rise of factories. The use of coal and other fossil fuels as energy sources also transformed transportation, with the development of steam-powered locomotives and steamships. The industrial revolution also brought about urbanization, as people moved from rural areas to cities in search of jobs. However, it also led to environmental degradation, social inequality, and labor exploitation.",
    "The Great Wall of China is a series of fortifications made of stone, brick, tamped earth, wood, and other materials, generally built along an east-to-west line across the historical northern borders of China to protect the Chinese states and empires against the raids and invasions of the various nomadic groups of the Eurasian Steppe. Several walls were built over millennia to protect the Chinese borders, with some sections dating back as far as the 7th century BC. The most well-known section is the Ming Wall, which was built during the Ming dynasty (1368–1644) to protect against the raids of the Mongol tribes. The wall stretches over 13,000 miles (21,196 kilometers) and is one of the most iconic symbols of China. It is a UNESCO World Heritage site and a popular tourist attraction.",
    "To be, , or not to be, that is the question: Whether 'tis nobler in the mind to suffer The slings and arrows of outrageous fortune, Or to take arms against a sea of troubles, And by opposing end them. To die, to sleep; No more; and by a sleep to say we end The heart-ache and the thousand natural shocks That flesh is heir to, 'tis a consummation Devoutly to be wish'd. To die, to sleep; To sleep, perchance to dream: ay, there's the rub; For in that sleep of death what dreams may come, When we have shuffled off this mortal coil, Must give us pause: there's the respect That makes calamity of so long life; For who would bear the whips and scorns of time, The oppressor's wrong, the proud man's contumely, The pangs of disprized love, the law's delay, The insolence of office, and the spurns That patient merit of th' unworthy takes, When he himself might his quietus make With a bare bodkin? who would fardels bear, To grunt and sweat under a weary life,",
    'Backpropagation is the method used in artificial neural networks to calculate a gradient that is needed in the calculation of the weights to be used in the network. It is a key part of the training of neural networks. The term "backpropagation" is often used as a synonym for the entire training procedure, but it actually refers only to the calculation of the gradient. The term was coined by Paul Werbos in 1974, but the idea was independently discovered by several researchers in the 1960s and 1970s. The backpropagation algorithm works by propagating the error backward from the output layer to the input layer, adjusting the weights of the neurons in the network to minimize the error. This process is repeated iteratively until the network converges to a solution that minimizes the error. The backpropagation algorithm is a key part of the training of neural networks, and it is used in a wide range of applications, including image and speech recognition, natural language processing, and reinforcement learning.',
    "Paul Gilbert owns a small bakery in the heart of Paris. He has been baking bread for over 20 years and is known for his delicious croissants and baguettes. His bakery is a popular spot for locals and tourists alike, who come to enjoy the fresh pastries and warm atmosphere. Paul takes pride in using only the finest ingredients in his baking, and he is always experimenting with new recipes to keep his offerings fresh and exciting. Despite the challenges of running a small business in a competitive market, Paul remains passionate about his craft and continues to bring joy to his customers with every bite.",
]


inputs = tokenize(texts)
batch_chunk_char_spans, batch_chunk_token_spans = chunk_by_sentences(
    inputs, chunk_size=64
)
print_chunks(texts, batch_chunk_char_spans)

embeddings = forward(inputs)
chunked_embeddings = apply_late_chunking(embeddings, batch_chunk_token_spans)


def run_query(query: str):
    query_embedding = forward(tokenize([query]), task="retrieval.query").mean(dim=1)
    results = find_k_most_similar_chunks(
        texts, batch_chunk_char_spans, query_embedding, chunked_embeddings, k=5
    )

    print(f"\nQuery: {query}\n")
    for i, result in enumerate(results):
        print(f"Result {i + 1}: {result}")


query = "What is the wonder of the world in the Asian country of which Beijing is the capital?"
run_query(query)
