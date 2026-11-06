import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.util.Arrays;
import java.util.concurrent.Executors;
import java.util.concurrent.atomic.AtomicInteger;

// Java has no JSON parser in its standard library; this client reads the body as a String
// and checks the id field textually, so it does slightly LESS work than the others.
public class JavaClient {
    static final URI URL = URI.create("http://127.0.0.1:3999/data");
    static final int TOTAL = 30_000, CONCURRENCY = 50, WARMUP = 3_000;

    static long[] run(HttpClient client, int n) throws Exception {
        double[] lat = new double[n];
        AtomicInteger next = new AtomicInteger();
        HttpRequest req = HttpRequest.newBuilder(URL).GET().build();
        long start = System.nanoTime();
        try (var exec = Executors.newVirtualThreadPerTaskExecutor()) {
            for (int w = 0; w < CONCURRENCY; w++) {
                exec.submit(() -> {
                    int i;
                    while ((i = next.getAndIncrement()) < n) {
                        long t = System.nanoTime();
                        String body = client.send(req, HttpResponse.BodyHandlers.ofString()).body();
                        if (!body.contains("\"id\":42")) throw new IllegalStateException("bad body");
                        lat[i] = (System.nanoTime() - t) / 1e6;
                    }
                    return null;
                });
            }
        }
        double elapsed = (System.nanoTime() - start) / 1e9;
        Arrays.sort(lat);
        return new long[] { (long) (n / elapsed), Math.round(lat[n / 2] * 100), Math.round(lat[n * 99 / 100] * 100) };
    }

    public static void main(String[] args) throws Exception {
        HttpClient client = HttpClient.newBuilder().version(HttpClient.Version.HTTP_1_1).build();
        run(client, WARMUP);
        long[] r = run(client, TOTAL);
        System.out.printf("{\"name\":\"Java HttpClient (no JSON parse)\",\"rps\":%d,\"p50\":\"%.2f\",\"p99\":\"%.2f\"}%n", r[0], r[1] / 100.0, r[2] / 100.0);
        System.exit(0);
    }
}
