// EidolonThreadedClient.hpp: High-Performance Multi-Threaded Client Wrapper
// Offloads network polling and packet processing to a background worker thread.
// Safely transfers events to the main render thread via double-buffered queues.
// Strictly standard C++17 with zero third-party dependencies.

#ifndef EIDOLON_THREADED_CLIENT_HPP
#define EIDOLON_THREADED_CLIENT_HPP

#include "EidolonClient.hpp"

#include <atomic>
#include <chrono>
#include <functional>
#include <mutex>
#include <thread>
#include <vector>

namespace eidolon {

/**
 * Thread-safe client wrapper managing background network polling.
 */
class ThreadedClient {
public:
    ThreadedClient()
        : m_running(false)
    {
    }

    ~ThreadedClient() {
        stop_worker();
    }

    // Non-copyable
    ThreadedClient(const ThreadedClient&) = delete;
    ThreadedClient& operator=(const ThreadedClient&) = delete;

    /**
     * Connects to the authoritative server and starts the background polling thread.
     */
    bool connect(
        const std::string& host,
        uint16_t port,
        uint64_t account_id,
        uint64_t ticket_high,
        uint64_t ticket_low,
        uint32_t poll_interval_ms = 10)
    {
        stop_worker();

        if (!m_client.connect(host, port, account_id, ticket_high, ticket_low)) {
            return false;
        }

        m_poll_interval_ms = poll_interval_ms;
        m_running = true;
        m_worker = std::thread(&ThreadedClient::worker_loop, this);
        return true;
    }

    /**
     * Disconnects from the server and terminates the worker thread.
     */
    void disconnect() {
        stop_worker();
        m_client.disconnect();
    }

    /**
     * Checks if the underlying client is connected.
     */
    bool is_connected() const {
        return m_client.is_connected();
    }

    /**
     * Drains all queued events to the calling (main/render) thread.
     */
    void drain_events(const std::function<void(const EidolonEvent&)>& handler) {
        std::vector<EidolonEvent> events_to_process;
        {
            std::lock_guard<std::mutex> lock(m_queue_mutex);
            events_to_process.swap(m_event_queue);
        }

        for (const auto& evt : events_to_process) {
            handler(evt);
        }
    }

    /**
     * Accessor to underlying client for sending commands.
     */
    Client& client() {
        return m_client;
    }

    const Client& client() const {
        return m_client;
    }

private:
    void worker_loop() {
        while (m_running.load(std::memory_order_relaxed)) {
            m_client.poll_events([this](const EidolonEvent& evt) {
                std::lock_guard<std::mutex> lock(m_queue_mutex);
                m_event_queue.push_back(evt);
            });

            std::this_thread::sleep_for(std::chrono::milliseconds(m_poll_interval_ms));
        }
    }

    void stop_worker() {
        if (m_running.load()) {
            m_running.store(false);
            if (m_worker.joinable()) {
                m_worker.join();
            }
        }
    }

    Client m_client;
    std::thread m_worker;
    std::atomic<bool> m_running;
    uint32_t m_poll_interval_ms = 10;
    std::mutex m_queue_mutex;
    std::vector<EidolonEvent> m_event_queue;
};

} // namespace eidolon

#endif // EIDOLON_THREADED_CLIENT_HPP
