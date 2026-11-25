package rs.synctube

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File

class LibraryTest {
    @get:Rule val tmp = TemporaryFolder()

    private val day = 24L * 3600
    private val now = 1_800_000_000L

    private fun id(n: Int) = "video%06d".format(n)
    private fun song(n: Int) = Song(id(n), "Pesma $n", 200_000)
    private fun track(n: Int) = Track("t$n", id(n), "Pesma $n", 200_000, "Ana")
    private fun ids(l: List<Song>) = l.map { it.youtubeId }

    // ----- score -----

    @Test fun scoresOwnSongsHigherAndFade() {
        val l = Library()
        l.record(song(1), false, now)
        l.record(song(2), true, now)
        fun at(n: Int, t: Long) = l.history.songs.getValue(id(n)).scoreAt(t)
        assertEquals(1.0, at(1, now), 0.0)
        assertEquals(2.5, at(2, now), 0.0)
        assertEquals(1.25, at(2, now + 14 * day), 1e-9)
        l.record(song(2), false, now + 14 * day)
        assertEquals(2.25, at(2, now + 14 * day), 1e-9)
        assertEquals(2, l.history.songs.getValue(id(2)).plays)
        assertEquals(1, l.history.songs.getValue(id(2)).ownPlays)
        assertEquals("a clock that went back does not grow the score", 2.25, at(2, now), 1e-9)
    }

    @Test fun historyIsCappedAndKeepsFavourites() {
        val l = Library()
        l.record(song(0), true, now)
        for (n in 1..Library.MAX_HISTORY) l.record(song(n), n % 2 == 0, now + n)
        assertTrue(l.history.songs.size <= Library.MAX_HISTORY)
        assertTrue(id(Library.MAX_HISTORY) in l.history.songs)
    }

    // ----- seeds -----

    @Test fun seedsTakeFavouritesAndRandomRest() {
        val l = Library()
        for (n in 1..10) repeat(n) { l.record(song(n), false, now) }
        val s = l.seeds(emptyList(), now) { 0.0 }.map { it.song.youtubeId }
        assertEquals(listOf(id(10), id(9), id(8)), s.take(3))
        assertEquals(5, s.toSet().size)
        assertEquals(5, l.seeds(emptyList(), now) { 0.999_999 }.map { it.song.youtubeId }.toSet().size)
    }

    @Test fun randomSeedsFollowTheWeight() {
        val l = Library()
        for (n in 1..3) repeat(100) { l.record(song(n), false, now) }
        repeat(9) { l.record(song(4), false, now) }
        l.record(song(5), false, now)
        // 0.5 of a total weight of 10 falls on the song that has 9 of it.
        val s = l.seeds(emptyList(), now) { 0.5 }
        assertEquals(id(4), s[3].song.youtubeId)
        assertEquals(id(5), s[4].song.youtubeId)
    }

    @Test fun seedsCountPlaylistsAndFallBackToTheQueue() {
        val l = Library()
        assertTrue(l.seeds(emptyList(), now).isEmpty())
        assertEquals(2, l.seeds(listOf(track(1), track(2)), now).size)
        l.record(song(1), false, now)
        val p = l.create("Moja")!!
        l.add(p, listOf(song(2)))
        // With a taste of one's own the queue no longer matters.
        val s = l.seeds(listOf(track(3)), now)
        assertEquals("in a playlist beats heard once", listOf(id(2), id(1)), s.map { it.song.youtubeId })
        assertEquals(2.0, s[0].weight, 0.0)
    }

    // ----- rank -----

    @Test fun rankPrefersSharedSongsAndFilters() {
        val l = Library()
        l.record(song(50), false, now - day)
        l.record(song(51), false, now - 8 * day)
        val p = l.create("Moja")!!
        l.add(p, listOf(song(60)))
        val short = song(70).copy(durationMs = 30_000)
        val long = song(71).copy(durationMs = 3_600_000)
        val twin = song(72).copy(title = "PESMA 2 (Official Video) [4K]")
        val seeds = listOf(Seed(song(1), 2.0), Seed(song(9), 2.0))
        val mixes = listOf(
            Mix(2.0, listOf(song(1), song(2), song(3), song(50), song(51), song(60), short, long)),
            Mix(2.0, listOf(song(9), song(4), song(3), song(80), twin)),
        )
        val out = ids(l.rank(seeds, mixes, listOf(track(80)), now))
        assertEquals("in both mixes", id(3), out[0])
        assertEquals(setOf(id(2), id(3), id(4), id(51)), out.toSet())
        assertEquals(4, out.size)
    }

    @Test fun rankUsesSeedScoreAndPosition() {
        val weak = Mix(1.0, listOf(song(1), song(2)))
        val strong = Mix(1.5, listOf(song(3), song(4)))
        // 1.5/3, 1.5/4, 1/3, 1/4
        val out = Library().rank(emptyList(), listOf(weak, strong), emptyList(), now)
        assertEquals(listOf(id(3), id(4), id(1), id(2)), ids(out))
    }

    @Test fun rankIsCapped() {
        val mix = Mix(1.0, (100 until 200).map(::song))
        assertEquals(Library.MAX_RECS, Library().rank(emptyList(), listOf(mix), emptyList(), now).size)
    }

    @Test fun recsGoStale() {
        val l = Library()
        assertTrue(l.recsStale(now))
        l.setRecs(listOf(song(1)), now)
        assertFalse(l.recsStale(now + 3600))
        assertTrue(l.recsStale(now + 13 * 3600))
        assertTrue("the clock went back", l.recsStale(now - 10))
    }

    @Test fun titleKeyIgnoresBrackets() {
        assertEquals("ana", Library.titleKey("Ana  (Official Video) [4K]"))
        assertEquals("(intro)", Library.titleKey("(Intro)"))
    }

    // ----- playlists -----

    @Test fun playlistsSkipDuplicatesAndCap() {
        val l = Library()
        assertNull(l.create("   "))
        val p = l.create(" Žurka ")!!
        assertEquals("same name, same playlist", p, l.create("žurka"))
        assertEquals(Added(2, 0), l.add(p, listOf(song(1), song(2), song(1))))
        assertEquals(Added(0, 0), l.add(p, listOf(song(2))))
        assertEquals("not a video id", Added(0, 0), l.add(p, listOf(song(1).copy(youtubeId = "x"))))
        assertEquals(Added(498, 99), l.add(p, (3 until 600).map(::song)))
        assertEquals(Library.MAX_PLAYLIST, l.get(p)!!.tracks.size)
        assertNull(l.add("nema", listOf(song(1))))
    }

    @Test fun playlistsEdit() {
        val l = Library()
        val p = l.create("A")!!
        l.add(p, (1..3).map(::song))
        l.moveSong(p, id(3), 0)
        l.moveSong(p, id(1), 99)
        l.moveSong(p, "nema", 0)
        l.remove(p, id(2))
        assertEquals(listOf(id(3), id(1)), ids(l.get(p)!!.tracks))
        l.rename(p, "B")
        l.rename(p, " ")
        assertEquals("B", l.get(p)!!.name)
        l.rename(p, "𝄞".repeat(100))
        assertEquals("not cut inside a character", "𝄞".repeat(Library.MAX_NAME_CHARS), l.get(p)!!.name)
        l.delete(p)
        assertTrue(l.playlists.isEmpty())
    }

    @Test fun learnsDurations() {
        val l = Library()
        val p = l.create("A")!!
        l.add(p, listOf(song(1).copy(durationMs = null)))
        assertFalse(l.learnDurations(listOf(track(2))))
        assertTrue(l.learnDurations(listOf(track(1))))
        assertEquals(200_000L, l.get(p)!!.tracks[0].durationMs)
        assertFalse(l.learnDurations(listOf(track(1))))
    }

    // ----- files -----

    @Test fun survivesARestart() {
        val dir = tmp.newFolder()
        val l = Library.load(dir)
        val p = l.create("Moja")!!
        l.add(p, listOf(song(1).copy(channel = "Kanal")))
        l.record(song(2), true, now)
        l.setRecs(listOf(song(3)), now)
        l.savePlaylists()
        l.saveHistory()
        val back = Library.load(dir)
        assertEquals(l.playlists, back.playlists)
        assertEquals(l.history, back.history)
        back.clearHistory()
        back.saveHistory()
        val again = Library.load(dir)
        assertFalse(again.hasHistory)
        assertTrue(again.recs.isEmpty())
        assertEquals("playlists stay", l.playlists, again.playlists)
        assertEquals(setOf("playlists.json", "history.json"), dir.list()!!.toSet())
    }

    @Test fun readsTheDesktopFormat() {
        val dir = tmp.newFolder()
        File(dir, "playlists.json").writeText(
            """[{"id":"a","name":"Moja","tracks":[{"youtube_id":"${id(1)}","title":"T","duration_ms":null,"channel":null},
                {"youtube_id":"${id(1)}","title":"again"}],"later":1}]""",
        )
        File(dir, "history.json").writeText(
            "﻿" + """{"songs":{"${id(2)}":{"title":"X","duration_ms":5,"plays":3,"own_plays":1,"score":2.5,"last_unix":9}},
                "recs":{"at_unix":7,"items":[]}}""",
        )
        val l = Library.load(dir)
        assertEquals(listOf(Song(id(1), "T")), l.playlists.single().tracks)
        assertEquals(Heard("X", 5, 3, 1, 2.5, 9), l.history.songs[id(2)])
        assertEquals(7L, l.history.recs.atUnix)
    }

    @Test fun damagedFileIsSetAside() {
        val dir = tmp.newFolder()
        val file = File(dir, "playlists.json")
        for (text in listOf("{not json", "[[[")) {
            file.writeText(text)
            val l = Library.load(dir)
            assertTrue(l.playlists.isEmpty())
            l.create("Nova")
            l.savePlaylists()
        }
        val bad = dir.listFiles()!!.filter { it.extension == "bad" }.map { it.readText() }.toSet()
        assertEquals(setOf("{not json", "[[["), bad)
        assertEquals(1, Library.load(dir).playlists.size)
    }

    @Test fun unreadableFileIsNeverOverwritten() {
        val dir = tmp.newFolder()
        // A folder in its place cannot be read as a file.
        val inside = File(dir, "history.json").apply { mkdirs() }.resolve("x").apply { writeText("x") }
        val l = Library.load(dir)
        l.record(song(1), true, now)
        l.saveHistory()
        assertTrue(inside.exists())
    }

    // ----- batches -----

    @Test fun batchesFitInAMessage() {
        val songs = (0 until 500).map { song(it).copy(title = "ž\"".repeat(100)) }
        val b = Library.batches(songs)
        assertTrue(b.size > 1)
        for (batch in b) {
            val size = Wire.encode(ClientMsg.AddResolved(Long.MAX_VALUE, batch)).toByteArray().size
            assertTrue("$size", size < 41 * 1024)
        }
        assertEquals(songs.map { it.youtubeId }, b.flatten().map { it.youtubeId })
        assertTrue(Library.batches(emptyList()).isEmpty())
        assertEquals(1, Library.batches(listOf(song(1))).size)
    }

    // ----- listening -----

    @Test fun listeningCountsOnceAfterThirtySeconds() {
        val t = ListenTimer()
        assertEquals(listOf(30L), (0L until 100).filter { t.tick("a", true, 200_000, it * 1000) })
        // A new song starts over; a short one needs half of it.
        assertEquals(listOf(110L), (100L until 140).filter { t.tick("b", true, 20_000, it * 1000) })
        // The first one again is another play.
        assertEquals(listOf(230L), (200L until 260).filter { t.tick("a", true, null, it * 1000) })
    }

    @Test fun pausesAndGapsDoNotCount() {
        val t = ListenTimer()
        t.tick("a", true, null, 0)
        // The phone slept for an hour.
        assertFalse(t.tick("a", true, null, 3_600_000))
        for (s in 0L until 60) assertFalse(t.tick("a", false, null, 3_601_000 + s * 1000))
        assertFalse(t.tick("a", true, null, 3_700_000))
        assertEquals(ListenTimer.MAX_STEP_MS, t.heardMs)
        assertFalse(t.tick(null, true, null, 3_701_000))
    }
}
