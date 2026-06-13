// cgx-fixture: async/await (TypeScript)
// Covers: calls:async (await on async fn), suspends property at await sites,
//         Promise chains as calls:async, async method on class

export async function fetchData(url: string): Promise<string> {
    const raw = await httpGet(url);        // calls:async, suspension point
    const parsed = await parseResponse(raw);  // calls:async, suspension point
    return parsed;
}

async function httpGet(url: string): Promise<string> {
    if (!url) throw new Error("empty url");
    return `response from ${url}`;
}

async function parseResponse(raw: string): Promise<string> {
    if (!raw) throw new Error("empty response");
    return raw.toUpperCase();
}

export class ApiClient {
    private baseUrl: string;

    constructor(baseUrl: string) {
        this.baseUrl = baseUrl;
    }

    async get(path: string): Promise<string> {
        const url = `${this.baseUrl}/${path}`;
        return fetchData(url);  // calls:async (implicit await in async method)
    }

    async post(path: string, body: string): Promise<string> {
        const url = `${this.baseUrl}/${path}`;
        return httpPost(url, body);   // calls:async
    }
}

async function httpPost(url: string, body: string): Promise<string> {
    if (!url || !body) throw new Error("invalid request");
    return `posted ${body.length} bytes to ${url}`;
}

/// Sequential awaits — each is a separate suspension point.
export async function sequentialAwaits(url: string): Promise<string> {
    const a = await httpGet(url);   // suspension point 1
    const b = await httpGet(url);   // suspension point 2
    return a + b;
}

/// await in a try block — suspension point inside exception context.
export async function awaitInTry(url: string): Promise<string> {
    try {
        const result = await httpGet(url);   // calls:async, always (inside try body)
        return result;
    } catch (e) {
        return "error";
    }
}

/// Promise.all — awaits multiple promises — calls:async on the await.
export async function parallel(urls: string[]): Promise<string[]> {
    const promises = urls.map(url => httpGet(url));
    return Promise.all(promises);  // calls:async on the await of Promise.all
}
